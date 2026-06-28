package workspace

import (
	"context"
	"fmt"
	"os"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/command"
	"rune/pkg/docstate"
	"rune/pkg/editor/keybind"
	"rune/pkg/terminal"
	"rune/pkg/ui/components/breadcrumb"
	"rune/pkg/ui/components/chat"
	dictcomp "rune/pkg/ui/components/dictation"
	"rune/pkg/ui/components/filetree"
	"rune/pkg/ui/components/footer"
	"rune/pkg/ui/components/markdownedit"
	"rune/pkg/ui/components/opentabs"
	searchcomp "rune/pkg/ui/components/search"
	"rune/pkg/ui/components/textedit"
	"rune/pkg/ui/components/title"
	"rune/pkg/ui/help"
	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"
	"rune/pkg/vfs"
)

// ---- Pane focus enum ----

type pane int

const (
	paneTree pane = iota
	paneTabs
	paneCenter
	paneTitle
	paneChat
	paneSearch
)

func (p pane) isLeft() bool   { return p == paneTree || p == paneTabs }
func (p pane) isCenter() bool { return p == paneCenter || p == paneTitle || p == paneSearch }

// ---- Drag state ----

type dragState int

const (
	dragNone dragState = iota
	dragLeft
	dragRight
)

// ---- Layout constants ----

const defaultLeftPaneW = 22
const defaultRightPaneW = 38

const (
	minLeftPaneW  = 16
	minRightPaneW = 20
	minCenterW    = 24
)

const tabLimit = 10

// ---- Message types ----

// ErrMsg signals a non-fatal I/O error to the workspace.
type ErrMsg struct{ Err error }

// dirChangedMsg signals the watched directory changed on disk.
type dirChangedMsg struct{}

// fileWatchReadError signals fsnotify detected a write but the file could not
// be re-read (deleted, moved, or permission denied).
type fileWatchReadError struct {
	path string
	err  error
}

// StoreReadyMsg is emitted when the docstate store has been opened.
type StoreReadyMsg struct {
	Store   *docstate.Store
	Warning string
}

// AutosaveSettledMsg is emitted after a VFS snapshot goroutine completes.
// Exported so the fuzz driver can detect autosave completion for DL1 checks.
// err is non-nil when the snapshot write failed (surfaced to the user; the
// journal remains the durable record).
type AutosaveSettledMsg struct {
	gen uint64
	err error
}

// pendingFlushMsg is returned by the debounce goroutine. The handler checks
// gen == m.flushGen before firing snapshotCmd so only the latest flush wins.
type pendingFlushMsg struct{ gen uint64 }

// ---- Data-loss action disambiguation ----

// actionKind records WHY the dirty-buffer guard was raised, so the guard
// response (and the async Save round-trip) knows whether to close this tab,
// quit, or do nothing.
type actionKind int

const (
	actionNone  actionKind = iota
	actionClose            // raised by requestCloseCurrent (^w)
	actionQuit             // raised by ConfirmQuitMsg (^C^C)
	actionEvict            // raised when a dirty tab must be evicted to open a new file
)

// pendingDataLoss carries the state a raised dirty guard must survive across the
// async Save→FileSavedMsg round-trip (§5.5). For actionQuit "Save", saveLeft
// counts the outstanding per-tab materialize acks before teardown; the first
// failure clears the whole action so every buffer is kept. For actionEvict,
// victim identifies the tab to close and pendingOpenPath is the file to open
// once the victim is dealt with; requestID correlates the background save ack.
type pendingDataLoss struct {
	kind            actionKind
	saveLeft        int
	victim          opentabs.TabHandle // eviction target (actionEvict)
	pendingOpenPath string             // file to open after eviction (actionEvict)
	requestID       string             // correlates evict-save ack (actionEvict + Save)
}

// ---- Guard options ----

// dataLossGuardOptions drives the dirty-buffer prompt. Cancel is LAST so that
// Escape (which the footer resolves to the final option) means Cancel, never
// Discard — Escape must never lose data (Fix 7 §1).
var dataLossGuardOptions = []footer.GuardOption{
	{Key: 's', Response: footer.DataLossSave},
	{Key: 'd', Response: footer.DataLossDiscard},
	{Key: 0, Response: footer.DataLossCancel}, // Esc → guardOptions[len-1] = Cancel
}

// ---- Model ----

type Model struct {
	totalWidth, totalHeight int
	title                   title.Model
	breadcrumb              breadcrumb.Model
	filetree                filetree.Model
	opentabs                opentabs.Model
	editor                  markdownedit.Model
	footer                  footer.Model
	focus                   pane
	leftVisible             bool
	leftPaneW               int
	chat                    chat.Model
	rightVisible            bool
	rightPaneW              int
	drag                    dragState
	err                     error
	keys                    keymap.Bindings
	styles                  styles.Styles

	// Help document — rendered once from the keymap; shown read-only under help.DocPath.
	helpContent string

	// In-file search bar (task 8)
	search searchcomp.Model

	// Dictation (D16)
	dict dictcomp.Model

	// Directory watching
	watchedDir  string
	cancelWatch context.CancelFunc

	// File ownership (D12). view is the single settled source of truth for which
	// document is displayed (kind + path + docID + baseline) — see docview.go. The
	// editor buffer always corresponds to it; it changes only at a settled
	// transition or a gen-matched load success.
	view       docView
	activeSave SaveIdentity

	// pendingLoad gates the center pane blank during an in-flight async file
	// load (preserving 16138bd's anti-flash) WITHOUT destroying the editor
	// buffer or identity — so a failed load falls back to the previous,
	// fully-consistent document and ⌘S can never write blank over a real file.
	// docID/path carry the incoming tab so finalize() can mark it active during
	// a close transition without forging a real identity. active is an
	// out-of-band validity bit (§1.7), never a sentinel on docID/path.
	pendingLoad pendingLoad

	// Pending data-loss action — set when a dirty guard is raised so that the
	// guard response handler knows to close a tab (^w) or quit (^C^C) after
	// saving/discarding. Never persisted across guard sessions.
	pendingDataLoss pendingDataLoss

	// Persistence (docstate). The active doc's VFS id lives in m.view.DocID().
	store     *docstate.Store
	chatDocID int64  // reserved chat sentinel doc
	flushGen  uint64 // generation counter for debounced VFS autosave
	loadGen   uint64 // monotonic load-request token; a FileLoadedMsg installs its
	//                  content+identity only while its Gen == m.pendingLoad.gen, so a
	//                  superseded/out-of-order read can never display the wrong doc.
	//                  Never reset (generations are never reused).

	// fs is the filesystem shim for all .md disk I/O (read/write/rename/stat/
	// readdir). A nil fs means the production default (vfs.Disk); the session
	// fuzzer injects a shared vfs.Mem via WithFS so the whole session runs in
	// memory. Access it through m.fsys(), never the raw field.
	fs vfs.FS

	// Startup configuration (set once, read by Init).
	workDir      string   // absolute path or "." passed via -w
	initialFiles []string // files to open on first Init
	initErr      error    // non-nil when workDir fallback was triggered
}

// pendingLoad records an in-flight asynchronous file load. See the Model field
// of the same name for the full contract. gen is the load-request token (from
// m.loadGen) this load awaits; a FileLoadedMsg/FileLoadErrorMsg is applied only
// when its Gen == gen AND active, so only the latest-requested load can settle.
type pendingLoad struct {
	gen    uint64
	docID  int64
	path   string
	active bool
}

// New constructs the workspace page. New does NOT call Init — the runtime calls
// Init when the page becomes active.
func New(keys keymap.Bindings, st styles.Styles, reg command.Registry, resolver keybind.Resolver, caps terminal.TermCaps, workDir string, initialFiles []string) Model {
	var initErr error
	if workDir == "" {
		if wd, err := os.Getwd(); err == nil {
			workDir = wd
		} else {
			initErr = fmt.Errorf("failed to get working directory: %w", err)
			workDir = "."
		}
	}
	m := Model{
		title: title.New("Untitled", keys, st,
			textedit.WithRegistry(reg),
			textedit.WithResolver(resolver),
		),
		breadcrumb: breadcrumb.New(st, nil),
		filetree:   filetree.New(keys, st),
		opentabs:   opentabs.New(keys, st),
		editor: markdownedit.New(keys, st, caps,
			markdownedit.WithRegistry(reg),
			markdownedit.WithResolver(resolver),
		).SetRoot(workDir), // static base #2 for relative-ref resolution (launch CWD)
		footer: footer.New(keys, st),
		chat:   chat.New(keys, st, reg, resolver, caps),
		search: searchcomp.New(keys, st,
			textedit.WithRegistry(reg),
			textedit.WithResolver(resolver),
		),
		dict:         dictcomp.New(),
		focus:        paneTree,
		leftVisible:  true,
		leftPaneW:    defaultLeftPaneW,
		rightVisible: false,
		rightPaneW:   defaultRightPaneW,
		keys:         keys,
		styles:       st,
		workDir:      workDir,
		initialFiles: initialFiles,
		initErr:      initErr,
		helpContent:  help.Document(keys),
	}
	if len(initialFiles) == 0 {
		// No files to open — register the initial untitled tab so the tab bar is
		// never empty. The store is not open yet, so this scratch starts with
		// docID==0; StoreReadyMsg upgrades it to a durable VFS doc.
		m, _ = m.CreateUntitled()
		m = m.setFocus(paneTree) // CreateUntitled sets paneCenter; restore startup default
	} else {
		// Files to open: seed the load overlay so Init's startup reads (issued with
		// per-file generations 1..N) are gen-correlated. The last file is the
		// awaited displayed doc (gen==N); earlier files open as tabs via the ungated
		// bookkeeping path. The seed lives here, not in Init, because Init returns
		// only a tea.Cmd and cannot retain the model.
		m.loadGen = uint64(len(initialFiles))
		m.pendingLoad = pendingLoad{
			gen:    m.loadGen,
			path:   initialFiles[len(initialFiles)-1],
			active: true,
		}
	}
	m = m.syncDictationAllowed()
	m = m.applyFocus()
	return m
}

// WithFS injects the filesystem shim used for all .md disk I/O. Production never
// calls it (the nil default resolves to vfs.Disk); the session fuzzer injects a
// shared vfs.Mem so load/save/rename/readdir run fully in memory. The same shim is
// pushed to the editor so its link/embed resolution + image reads see the SAME
// files the workspace serves (§1.4.9) — otherwise in-memory cross-links would all
// resolve as missing against real disk.
func (m Model) WithFS(fs vfs.FS) Model {
	m.fs = fs
	m.editor = m.editor.SetFS(fs)
	return m
}

// fsys returns the active filesystem shim, defaulting to real disk. Every file
// Cmd factory takes its result so the operation runs against the same backend
// the store resolves identity against.
func (m Model) fsys() vfs.FS {
	if m.fs == nil {
		return vfs.Disk{}
	}
	return m.fs
}

// Init is called once when the workspace page becomes active.
func (m Model) Init() tea.Cmd {
	cmds := []tea.Cmd{
		m.filetree.Init(),
		m.opentabs.Init(),
		m.editor.Init(),
		m.footer.Init(),
		m.chat.Init(),
		m.search.Init(),
		m.dict.Init(),
		loadDirCmd(m.fsys(), m.workDir),
		openStoreCmd(),
	}
	if m.initErr != nil {
		err := m.initErr
		cmds = append(cmds, func() tea.Msg { return footer.ShowErrorMsg{Text: err.Error()} })
	}
	for i, path := range m.initialFiles {
		// Startup reads carry per-file generations 1..N matching the overlay seeded
		// in New; the last (gen==N) becomes the displayed doc, earlier ones open
		// tabs. Issued directly (not beginLoad) because Init can't retain the gen
		// increment — so loadFileCmd has exactly two callers: beginLoad and Init.
		cmds = append(cmds, loadFileCmd(m.fsys(), context.Background(), path, uint64(i+1)))
	}
	return tea.Batch(cmds...)
}
