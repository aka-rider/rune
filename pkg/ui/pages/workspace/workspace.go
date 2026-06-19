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
	"rune/pkg/ui/components/textedit"
	"rune/pkg/ui/components/title"
	"rune/pkg/ui/help"
	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"
)

// ---- Pane focus enum ----

type pane int

const (
	paneTree   pane = iota
	paneTabs
	paneCenter
	paneTitle
	paneChat
)

func (p pane) isLeft() bool   { return p == paneTree || p == paneTabs }
func (p pane) isCenter() bool { return p == paneCenter || p == paneTitle }

// ---- Drag state ----

type dragState int

const (
	dragNone  dragState = iota
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
type AutosaveSettledMsg struct{ gen uint64 }

// pendingFlushMsg is returned by the debounce goroutine. The handler checks
// gen == m.flushGen before firing snapshotCmd so only the latest flush wins.
type pendingFlushMsg struct{ gen uint64 }

// ---- Data-loss action disambiguation ----

// pendingDataLossAction records WHY the dirty-buffer guard was raised so that
// DataLossDiscard / FileSavedMsg know whether to close just this tab or quit.
type pendingDataLossAction int

const (
	dataLossActionNone  pendingDataLossAction = iota
	dataLossActionClose                       // raised by requestCloseCurrent (^w)
	dataLossActionQuit                        // raised by ConfirmQuitMsg (^C^C)
)

// ---- Guard options ----

var quitGuardOptions = []footer.GuardOption{
	{Key: 's', Response: footer.DataLossSave},
	{Key: 'd', Response: footer.DataLossDiscard},
	{Key: 0, Response: footer.DataLossCancel}, // Esc → Cancel
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

	// Untitled buffer preserved across tab switches (the "" tab has no disk backing).
	untitledContent string
	untitledTitle   string

	// Dictation (D16)
	dict dictcomp.Model

	// Directory watching
	watchedDir  string
	cancelWatch context.CancelFunc

	// File ownership (D12)
	filePath   string
	cleanRev   uint64 // editor revision at last load or save
	activeSave SaveIdentity

	// Pending data-loss action — set when a dirty guard is raised so that the
	// guard response handler knows to close a tab (^w) or quit (^C^C) after
	// saving/discarding. Never persisted across guard sessions.
	pendingDataLoss pendingDataLossAction

	// Persistence (docstate)
	store     *docstate.Store
	docID     int64
	headSeq   int64  // most recent AppendEdit seq — co-captured for snapshot tagging (N5)
	chatDocID int64  // reserved chat sentinel doc
	flushGen  uint64 // generation counter for debounced VFS autosave

	// Startup configuration (set once, read by Init).
	workDir      string   // absolute path or "." passed via -w
	initialFiles []string // files to open on first Init
	initErr      error    // non-nil when workDir fallback was triggered
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
		),
		footer:       footer.New(keys, st),
		chat:         chat.New(keys, st, reg, resolver, caps),
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
		// never empty and CreateUntitled(true) has a "" tab to rename.
		m, _ = m.CreateUntitled(false)
		m.focus = paneTree // CreateUntitled sets paneCenter; restore startup default
	}
	m = m.syncDictationAllowed()
	m = m.applyFocus()
	return m
}

// Init is called once when the workspace page becomes active.
func (m Model) Init() tea.Cmd {
	cmds := []tea.Cmd{
		m.filetree.Init(),
		m.opentabs.Init(),
		m.editor.Init(),
		m.footer.Init(),
		m.chat.Init(),
		m.dict.Init(),
		loadDirCmd(m.workDir),
		openStoreCmd(),
	}
	if m.initErr != nil {
		err := m.initErr
		cmds = append(cmds, func() tea.Msg { return footer.ShowErrorMsg{Text: err.Error()} })
	}
	for _, path := range m.initialFiles {
		cmds = append(cmds, loadFileCmd(context.Background(), path))
	}
	return tea.Batch(cmds...)
}
