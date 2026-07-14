package workspace

import (
	"context"
	"fmt"
	"os"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/ai"
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
	"rune/pkg/ui/pages/workspace/mergemode"
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

// Message types (ErrMsg, dirChangedMsg, fileChangedMsg, fileWatchReadError,
// StoreReadyMsg, AutosaveSettledMsg, pendingFlushMsg) live in
// workspace_msgs.go. The guard sum type (guardState/guardKind/guardPhase),
// its option var declarations, and raiseGuardPrompt/clearGuardPrompt live in
// workspace_guard.go (A4 absorbed the former workspace_guardopts.go's
// actionKind/pendingDataLoss — deleted, folded into guardState — and
// deletedIntent, moved to workspace_deleted.go).

// ---- Model ----

type Model struct {
	totalWidth, totalHeight int
	title                   title.Model
	breadcrumb              breadcrumb.Model
	filetree                filetree.Model
	opentabs                opentabs.Model
	editor                  markdownedit.Model
	merge                   mergemode.State // active 3-way merge resolver (§4); mergemode.IsActive(m.merge) is the merge discriminant
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

	// dirWatcher is the directory-watch shim used by startWatch. A nil
	// dirWatcher means the production default (FSNotifyWatcher); every test
	// constructor in this package and the session fuzzer inject NoopWatcher
	// via WithWatcher so no goroutine or OS watch descriptor is ever opened
	// outside production. Access it through m.watcher(), never the raw field.
	dirWatcher Watcher

	// memoryStore is true when the rootChooser's "None" option was picked:
	// Init opens the docstate recovery store as :memory: (openStoreMemory)
	// instead of workDir/.rune/rune.db (openStore) — no .rune directory is
	// ever created. The file tree/breadcrumb/link-resolution root is
	// unaffected; only the store-open call in Init branches on this. Set via
	// WithMemoryStore, defaults false (disk-backed, today's behavior).
	memoryStore bool

	// File ownership (D12). view is the single settled source of truth for which
	// document is displayed (kind + path + docID) — see docview.go. The
	// editor buffer always corresponds to it; it changes only at a settled
	// transition or a gen-matched load success. WP5: no more per-tab size/
	// mtime baseline cache — every divergence decision is driven by
	// docstate.SyncState (Sync/Probe/Load's own comparison of recorded
	// facts), never a workspace-side fingerprint.
	view       docView
	activeSave SaveIdentity

	// guard is workspace's semantic guard state machine (A3/A4) — kind/phase
	// track ONLY which guard, if any, currently owns the footer's modal
	// prompt (see guardState's doc comment, workspace_guard.go, for the
	// multi-field/critic-R1 design). Populated exclusively by
	// raiseGuardPrompt/clearGuardPrompt, the same chokepoint that owns every
	// footer.SetGuard call (A2). The intent payloads for each guard kind
	// (conflict/deleted/raced/trash/close/evict/quit) migrate into
	// dedicated guardState sub-fields one guard kind per commit; until a
	// kind migrates, its intent still lives in the pending* fields below.
	guard guardState

	// diskChangedHint is true when a cheap stat-on-focus (G) detects that the
	// current file changed on disk relative to its view baseline. The footer shows
	// a passive hint; no modal is raised.
	diskChangedHint bool

	// pendingLoad gates the center pane blank during an in-flight async file
	// load (preserving 16138bd's anti-flash) WITHOUT destroying the editor
	// buffer or identity — so a failed load falls back to the previous,
	// fully-consistent document and ⌘S can never write blank over a real file.
	// docID/path carry the incoming tab so finalize() can mark it active during
	// a close transition without forging a real identity. active is an
	// out-of-band validity bit (§1.7), never a sentinel on docID/path.
	pendingLoad pendingLoad

	// guard.close/guard.evict/guard.quit (A4: migrated from the former
	// Model.pendingDataLoss) — set when a dirty guard is raised so that the
	// guard response handler knows to close a tab (^w), evict a background
	// victim, or quit (^C^C) after saving/discarding. Never persisted across
	// guard sessions.

	// pendingReopen holds a navigation request requestOpenPath deferred because
	// it targeted the exact file an in-flight interactive save is writing
	// (savingTarget, workspace_probe.go) — reloading it now could observe
	// the atomic-rewrite's post-rename inode before docstate.Bind re-stamps it,
	// orphaning the doc's history onto a fresh docID (§1.4.6). Flushed via
	// flushPendingReopen once that save settles. active is an out-of-band
	// validity bit (§1.7), never a sentinel on docID/path.
	pendingReopen pendingReopen

	// guard.conflict (conflictIntent, workspace_conflict.go — migrated to
	// guardState by A3) holds the identity (docID/path) and the conflicting
	// disk observation (freshObs) captured when a FileSaveErrorMsg{Conflict:
	// true} or a load-time/undo-unwind divergence is detected for the current
	// document — never the theirs/ancestor bytes themselves, which are
	// derived fresh at guard-raise or resolution time via GetBlob/Probe.
	// Consumed by DataLossSaveAnyway / DataLossDiscard / DataLossMerge guard
	// responses. Zero value = no pending conflict. Cleared on every guard
	// resolution.

	// guard.deleted (deletedIntent, migrated to guardState by A3) holds the
	// docID/path of the current document when its file is detected missing
	// on disk (deletion, or parent-dir removal). Raised by handleProbeResult
	// (workspace_probe.go — probeDocCmd's callers: dirChangedMsg / the flush
	// tick) and handleFileSaveErrorMsg (a save-time Missing outcome);
	// consumed by DataLossSaveAnyway (recreate) / DataLossDiscard (purge)
	// guard responses. Zero value = no pending deletion. (workspace_probe.go
	// / workspace_deleted.go)

	// guard.raced (racedIntent, migrated to guardState by A3) holds the two
	// competing observations (Saved/Fresh) when a Materialize commits via the
	// F5 swap-race path (MatResult{Committed: true, Raced: true}): our write
	// landed for real, but a concurrent writer's displaced bytes were
	// captured too. A DISTINCT guard from guard.conflict (critic R1) — never
	// routed through the fresh-probe [D]/[M] handlers, which would re-read
	// disk, find OUR already-committed bytes, and read Clean, silently
	// dissolving the guard. Consumed by DataLossKeepMine /
	// DataLossRestoreTheirs. Zero value = no pending race. (workspace_raced.go)

	// racedQueue holds raced-save outcomes for documents that were NOT
	// displayed when their Materialize ack arrived (evict/quit-batch saves, a
	// tab switched away mid-save): the guard raises the moment the doc is
	// next displayed (drainRacedQueue at load-settle). A race must never
	// resolve silently just because its tab was in the background (review
	// finding). Lazily allocated; nil means empty. (workspace_raced.go)
	racedQueue map[int64]racedIntent

	// Persistence (docstate). The active doc's VFS id lives in m.view.DocID().
	store     *docstate.Store
	chatDocID int64  // reserved chat sentinel doc
	flushGen  uint64 // generation counter for debounced VFS autosave
	loadGen   uint64 // monotonic load-request token; a FileLoadedMsg installs its
	//                  content+identity only while its Gen == m.pendingLoad.gen, so a
	//                  superseded/out-of-order read can never display the wrong doc.
	//                  Never reset (generations are never reused). Loads keep this
	//                  mechanism (rather than migrating onto epoch below) — Part IV
	//                  §WP6 leaves that choice to the worker; loadGen is already the
	//                  single, heavily-tested mechanism for load staleness.

	// epoch is workspace-OWNED, in-memory, and bumps on every NON-journaled
	// buffer transition: a load install, untitled/help switch, undo/redo
	// MoveUndoPos, a merge/discard resolve ReplaceAll, or a recovery install
	// (Part IV "the ticket + chokepoint"). Journaled edits (typing, dictation
	// chunks once applied, drained broadcasts) do NOT bump it — those are
	// already the CURRENT epoch's own content, not a wholesale replacement of
	// it. Never reset; staleness is a session concept, never persisted to
	// docstate. See viewTicket/applyViewResult (workspace_ticket.go).
	epoch uint64

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

// pendingReopen records a navigation request deferred by requestOpenPath. See
// the Model field of the same name for the full contract. A fresh navigation
// request always supersedes a stale deferral (requestOpenPath clears it
// unconditionally before arming a new one), so only the most recently
// requested reopen ever replays.
type pendingReopen struct {
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
	// Constructed here, not inside chat.New (§2.5 — components don't read
	// env): the page is the injection point, chat just receives the result.
	aiClient, aiClientErr := ai.NewClient()
	m := Model{
		title: title.New("Untitled", keys, st,
			textedit.WithRegistry(reg),
			textedit.WithResolver(resolver),
		),
		breadcrumb: breadcrumb.New(st).SetDir(workDir), // §1.4.9: inject launch dir, no per-render os.Getwd
		filetree:   filetree.New(keys, st),
		opentabs:   opentabs.New(keys, st),
		editor: markdownedit.New(keys, st, caps,
			markdownedit.WithRegistry(reg),
			markdownedit.WithResolver(resolver),
		).SetRoot(workDir), // static base #2 for relative-ref resolution (launch CWD)
		merge:  mergemode.New(keys, st),
		footer: footer.New(keys, st),
		chat:   chat.New(keys, st, reg, resolver, caps, aiClient, aiClientErr),
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
		// per-file generations 1..N) are gen-correlated. The first file (gen==1) is
		// the awaited displayed doc; later files open as tabs via the ungated
		// bookkeeping path. The seed lives here, not in Init, because Init returns
		// only a tea.Cmd and cannot retain the model. loadGen still tracks N (not 1)
		// because it's the monotonic base beginLoad increments from later, and must
		// stay >= N regardless of which gen pendingLoad awaits. Focus stays at the
		// struct-literal default (paneTree) here; handleFileLoadedMsg grants
		// paneCenter once the gen-1 load settles.
		m.loadGen = uint64(len(initialFiles))
		m.pendingLoad = pendingLoad{
			gen:    1,
			path:   initialFiles[0],
			active: true,
		}
	}
	m = m.applyFocus()
	return m
}

// WithFS injects the filesystem shim used for all .md disk I/O. pkg/ui.NewApp
// calls it once at construction with vfs.Disk{} (S6: one shared value, rather
// than workspace and store each independently nil-defaulting); the session
// fuzzer injects a shared vfs.Mem so load/save/rename/readdir run fully in
// memory. The same shim is pushed to the editor (link/embed resolution +
// image reads), the chat pane's display (its rendered replies can embed
// images too — without this it silently bypassed the injected FS and always
// hit real disk, even under the fuzzer's Mem FS) and, if the store is already
// wired, to the store too — otherwise a test or a future re-injection after
// StoreReadyMsg could strand the store on a stale/disconnected FS while the
// workspace serves a different one, and in-memory cross-links would all
// resolve as missing against real disk (§1.4.9).
func (m Model) WithFS(fs vfs.FS) Model {
	m.fs = fs
	m.editor = m.editor.SetFS(fs)
	m.chat = m.chat.SetFS(fs)
	if m.store != nil {
		m.store.UseFS(fs)
	}
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

// WithWatcher injects the directory-watcher implementation used by
// startWatch, mirroring WithFS. A nil dirWatcher (the default) resolves to
// the real FSNotifyWatcher; test constructors and the session fuzzer inject
// NoopWatcher instead.
func (m Model) WithWatcher(w Watcher) Model {
	m.dirWatcher = w
	return m
}

// watcher returns the active directory watcher, defaulting to the real
// fsnotify-backed implementation.
func (m Model) watcher() Watcher {
	if m.dirWatcher == nil {
		return FSNotifyWatcher{}
	}
	return m.dirWatcher
}

// WithMemoryStore switches Init's store-open call to docstate.OpenInMemory
// instead of docstate.Open(workDir) — the rootChooser's "None" option. No
// .rune directory is ever created; nothing survives past this process.
func (m Model) WithMemoryStore() Model {
	m.memoryStore = true
	return m
}

// Init is called once when the workspace page becomes active.
func (m Model) Init() tea.Cmd {
	storeCmd := openStore(m.fsys(), m.workDir)
	if m.memoryStore {
		storeCmd = openStoreMemory(m.fsys())
	}
	cmds := []tea.Cmd{
		m.filetree.Init(),
		m.opentabs.Init(),
		m.editor.Init(),
		m.footer.Init(),
		m.chat.Init(),
		m.search.Init(),
		m.dict.Init(),
		loadDirCmd(m.fsys(), m.workDir),
		storeCmd,
	}
	if m.initErr != nil {
		err := m.initErr
		cmds = append(cmds, func() tea.Msg { return footer.ShowErrorMsg{Text: err.Error()} })
	}
	for i, path := range m.initialFiles {
		// Startup reads carry per-file generations 1..N matching the overlay seeded
		// in New; the first (gen==1) becomes the displayed doc, later ones open
		// tabs. Issued directly (not beginLoad) because Init can't retain the gen
		// increment — so loadFileCmd has exactly two callers: beginLoad and Init.
		cmds = append(cmds, loadFileCmd(m.store, m.fsys(), context.Background(), path, uint64(i+1)))
	}
	return tea.Batch(cmds...)
}
