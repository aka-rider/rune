package workspace

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"time"

	"charm.land/bubbles/v2/key"
	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"
	dictengine "rune/pkg/dictation"
	"rune/pkg/docstate"
	"rune/pkg/editor/buffer"
	"rune/pkg/editor/cursor"
	"rune/pkg/editor/keybind"
	"rune/pkg/terminal"
	dictcomp "rune/pkg/ui/components/dictation"
	"rune/pkg/ui/components/breadcrumb"
	"rune/pkg/ui/components/chat"
	"rune/pkg/ui/components/title"
	"rune/pkg/ui/components/filetree"
	"rune/pkg/ui/components/footer"
	"rune/pkg/ui/components/markdownedit"
	"rune/pkg/ui/components/opentabs"
	"rune/pkg/ui/components/textedit"
	"rune/pkg/command"
	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"
)

type pane int

const (
	paneTree   pane = iota
	paneTabs
	paneCenter // editor text area focused
	paneTitle  // title input focused (D11)
	paneChat
)

func (p pane) isLeft() bool   { return p == paneTree || p == paneTabs }
func (p pane) isCenter() bool { return p == paneCenter || p == paneTitle }

// ErrMsg signals an I/O error to the workspace page.
type ErrMsg struct{ Err error }

// dirChangedMsg signals the watched directory contents changed on disk.
type dirChangedMsg struct{}

// fileWatchReadError signals that fsnotify reported a write but the file
// could not be read (deleted, moved, or permission denied).
type fileWatchReadError struct {
	path string
	err  error
}

const defaultLeftPaneW = 22
const defaultRightPaneW = 38

var quitGuardOptions = []footer.GuardOption{
	{Key: 's', Response: footer.DataLossSave},
	{Key: 'd', Response: footer.DataLossDiscard},
}

type dragState int

const (
	dragNone dragState = iota
	dragLeft
	dragRight
)

const (
	minLeftPaneW  = 16
	minRightPaneW = 20
	minCenterW    = 24
)


// StoreReadyMsg is emitted when the docstate store has been opened.
type StoreReadyMsg struct {
	Store   *docstate.Store
	Warning string
}

// pendingFlushMsg is emitted after the autosave debounce timer fires.
type pendingFlushMsg struct{ gen uint64 }

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

	// Dictation (D16)
	dict dictcomp.Model

	// Directory watching
	watchedDir  string
	cancelWatch context.CancelFunc

	// File ownership (D12)
	filePath            string
	cleanRev            uint64 // editor revision at last load or save
	pendingQuitAfterSave bool  // true when user chose Save in the dirty-quit guard
	activeSave          SaveIdentity

	// Persistence (docstate)
	store    *docstate.Store
	docID    int64
	flushGen uint64

	// Startup configuration (set once, read by Init).
	workDir      string   // absolute path or "." passed via -w
	initialFiles []string // files to open on first Init
	initErr      error    // non-nil when workDir fallback was triggered
}

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
		footer:       footer.New(keys, st).SetHelp(keys.HelpText()),
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
	}
	if len(initialFiles) == 0 {
		// No files to open — register the initial untitled tab so the tab bar
		// is never empty and CreateUntitled(true) has a "" tab to rename.
		m, _ = m.CreateUntitled(false)
		m.focus = paneTree // CreateUntitled sets paneCenter; restore startup default
	}
	m = m.syncDictationAllowed()
	m = m.applyFocus() // project initial focus so paneTree reaches the filetree at launch
	return m
}

func (m Model) currentDir() string {
	if m.watchedDir != "" {
		return m.watchedDir
	}
	cwd, err := os.Getwd()
	if err != nil {
		return "."
	}
	return cwd
}


func (m Model) recalcLayout() Model {
	contentH := m.totalHeight - m.footer.Height()
	if contentH < 0 {
		contentH = 0
	}

	leftW := 0
	if m.leftVisible {
		leftW = m.leftPaneW
	}
	rightW := 0
	if m.rightVisible {
		rightW = m.rightPaneW
	}
	centerW := m.totalWidth - leftW - rightW
	if centerW < 0 {
		centerW = 0
	}

	innerH := contentH - 2
	if innerH < 0 {
		innerH = 0
	}
	innerLeftW := leftW - 2
	if innerLeftW < 0 {
		innerLeftW = 0
	}
	innerCenterW := centerW - 2
	if innerCenterW < 0 {
		innerCenterW = 0
	}
	innerRightW := rightW - 2
	if innerRightW < 0 {
		innerRightW = 0
	}

	// Title occupies the first row inside the center pane border (D6)
	titleH := m.title.Height()
	editorH := innerH - titleH
	if editorH < 0 {
		editorH = 0
	}

	otH := m.opentabs.Height()
	ftH := innerH - otH
	if ftH < 4 {
		ftH = 4
	}
	m.filetree = m.filetree.SetSize(innerLeftW, ftH)
	m.opentabs = m.opentabs.SetSize(innerLeftW, otH)

	// Title is cell-grid only (D8)
	m.title = m.title.SetSize(innerCenterW, titleH)
	m.breadcrumb = m.breadcrumb.SetSize(centerW, 1)

	// topOffset = absolute row of the center pane's top border
	topOffset := 1
	if m.err != nil {
		topOffset = 2 // error line shifts pane down one row
	}

	// Editor gets SetRect: Y absorbs top border + title row (D7, D8)
	m.editor = m.editor.SetRect(textedit.Rect{
		X: leftW + 1,
		Y: topOffset + titleH, // border top(1) + title rows; topOffset already includes errline offset
		W: innerCenterW,
		H: editorH,
	})

	m.chat = m.chat.SetSize(innerRightW, innerH)
	m.footer = m.footer.SetSize(m.totalWidth, m.footer.Height())

	// Filetree offset: 1 col inside the left border, 1 row inside top border
	m.filetree = m.filetree.SetOffset(1, 1)

	return m
}

func (m Model) paneAtPoint(x, y int) (pane, bool) {
	contentH := m.totalHeight - m.footer.Height()
	if y >= contentH {
		return 0, false // footer
	}

	if m.leftVisible && x < m.leftPaneW {
		innerH := contentH - 2
		otH := m.opentabs.Height()
		ftH := innerH - otH
		if ftH < 4 {
			ftH = 4
		}
		if y > ftH {
			return paneTabs, true
		}
		return paneTree, true
	}
	rightStart := m.totalWidth - m.rightPaneW
	if m.rightVisible && x >= rightStart {
		return paneChat, true
	}

	// Distinguish title area from editor area within the center pane
	// Title is inside the border (y == topOffset), editor content is below
	topOffset := 1
	if m.err != nil {
		topOffset = 2
	}
	if y == topOffset {
		return paneTitle, true
	}
	return paneCenter, true
}

func (m Model) dividerAtPoint(x, y int) (dragState, bool) {
	contentH := m.totalHeight - m.footer.Height()
	if y < 0 || y >= contentH {
		return dragNone, false
	}

	if m.leftVisible {
		if x == m.leftPaneW-1 || x == m.leftPaneW {
			return dragLeft, true
		}
	} else {
		if x == 0 {
			return dragLeft, true
		}
	}

	if m.rightVisible {
		rightStart := m.totalWidth - m.rightPaneW
		if x == rightStart-1 || x == rightStart {
			return dragRight, true
		}
	} else {
		if x == m.totalWidth-1 {
			return dragRight, true
		}
	}

	return dragNone, false
}

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

// startSave begins a save operation for the current file (D12, D13).
func (m Model) startSave() (Model, tea.Cmd) {
	if m.filePath == "" || m.activeSave.InFlight {
		return m, nil
	}
	content := m.editor.Content()
	requestID := fmt.Sprintf("save-%v", time.Now().UnixNano())
	m.activeSave = SaveIdentity{
		RequestID:    requestID,
		SavedContent: []byte(content),
		InFlight:     true,
	}
	return m, saveFileCmd(m.filePath, content, requestID)
}

func (m Model) requestOpenPath(path string) (Model, tea.Cmd) {
	if path == m.filePath {
		return m, nil
	}
	return m, loadFileCmd(context.Background(), path)
}

func (m Model) requestCloseCurrent() (Model, tea.Cmd) {
	nextPath := m.opentabs.NextPath(m.filePath)
	if m.filePath == "" && nextPath == "" {
		// Sole untitled tab — nothing to switch to; keep it.
		return m, nil
	}
	return m.executeClose(m.filePath, nextPath)
}

func (m Model) executeClose(closePath, nextPath string) (Model, tea.Cmd) {
	var cmds []tea.Cmd
	m.opentabs = m.opentabs.CloseFile(closePath)
	if nextPath != "" {
		cmds = append(cmds, loadFileCmd(context.Background(), nextPath))
	} else {
		// Last tab closed — reset to a fresh untitled buffer (no auto-save;
		// the user explicitly chose to close the buffer).
		var createCmd tea.Cmd
		m, createCmd = m.CreateUntitled(false)
		cmds = append(cmds, createCmd)
	}
	return m, tea.Batch(cmds...)
}


func (m Model) handleUndo() (Model, tea.Cmd) {
	if m.store == nil {
		return m, nil
	}
	surface, edits, cursorsBefore, ok := m.store.UndoTarget()
	if !ok {
		return m, nil
	}
	var cmd tea.Cmd
	switch surface {
	case "main":
		m.editor, cmd = m.editor.ApplyInverse(edits)
		m.editor = m.editor.SetCursors(cursorsBefore)
	case "title":
		m.title = m.title.ApplyInverse(edits)
		m.title = m.title.SetCursors(cursorsBefore)
	case "chat":
		m.chat = m.chat.ApplyInverse(edits)
		m.chat = m.chat.SetCursors(cursorsBefore)
	}
	// focus follows surface
	switch surface {
	case "main":
		m.focus = paneCenter
	case "title":
		m.focus = paneTitle
	case "chat":
		m.focus = paneChat
	}
	m = m.syncDictationAllowed()
	return m, cmd
}

func (m Model) handleRedo() (Model, tea.Cmd) {
	if m.store == nil {
		return m, nil
	}
	surface, edits, cursorsAfter, ok := m.store.RedoTarget()
	if !ok {
		return m, nil
	}
	var cmd tea.Cmd
	switch surface {
	case "main":
		m.editor, cmd = m.editor.Reapply(edits)
		m.editor = m.editor.SetCursors(cursorsAfter)
	case "title":
		m.title = m.title.Reapply(edits)
		m.title = m.title.SetCursors(cursorsAfter)
	case "chat":
		m.chat = m.chat.Reapply(edits)
		m.chat = m.chat.SetCursors(cursorsAfter)
	}
	switch surface {
	case "main":
		m.focus = paneCenter
	case "title":
		m.focus = paneTitle
	case "chat":
		m.focus = paneChat
	}
	m = m.syncDictationAllowed()
	return m, cmd
}

func (m Model) scheduleFlush(cmds *[]tea.Cmd) Model {
	m.flushGen++
	gen := m.flushGen
	store := m.store
	docID := m.docID
	content := m.editor.Content()
	if store != nil && docID > 0 {
		*cmds = append(*cmds, func() tea.Msg {
			time.Sleep(flushDelay)
			// snapshot in background
			if _, err := store.CreateSnapshot(docID, content, "local"); err != nil {
				// non-fatal: snapshot failed
				_ = err
			}
			return pendingFlushMsg{gen: gen}
		})
	}
	return m
}

// journalEdit appends an edit to the journal and schedules autosave.
// Call after DrainEdits returns non-empty edits.
func (m Model) journalEdit(surface string, edits []buffer.AppliedEdit, cursorsBefore, cursorsAfter []cursor.Cursor, cmds *[]tea.Cmd) Model {
	if m.store == nil || len(edits) == 0 {
		return m
	}
	if err := m.store.AppendEdit(surface, edits, cursorsBefore, cursorsAfter, surface); err != nil {
		// non-fatal journal error
		_ = err
	}
	if surface == "main" {
		m = m.scheduleFlush(cmds)
	}
	return m
}

func (m Model) syncCursorToFooter() Model {
	// Cursor line/col not yet tracked; footer shows no position.
	m.footer, _ = m.footer.Update(footer.UpdateCursorMsg{
		Line:      0,
		Col:       0,
		WordCount: 0,
	})
	return m
}

func (m Model) syncDictationAllowed() Model {
	m.footer = m.footer.SetDictationAllowed(m.focus == paneCenter || m.focus == paneChat)
	return m
}


// applyFocus projects the single focus authority (m.focus) onto every child's
// focus state. This is the ONLY place component focus is derived from the enum;
// it runs before every dispatch to children and on every Update exit, so the two
// representations can never disagree at a boundary the renderer or a child's
// Update can observe. Every child SetFocused is idempotent, so calling this each
// frame is free. (chat owns a nested prompt/display sub-focus internally — the
// same pattern one level down, self-consistent and out of scope here.)
func (m Model) applyFocus() Model {
	m.title = m.title.SetFocused(m.focus == paneTitle)
	m.filetree = m.filetree.SetFocused(m.focus == paneTree)
	m.opentabs = m.opentabs.SetFocused(m.focus == paneTabs)
	m.editor = m.editor.SetFocused(m.focus == paneCenter)
	m.chat = m.chat.SetFocused(m.focus == paneChat)
	return m
}

func (m Model) finalizeLayoutChange(cmds []tea.Cmd) (Model, tea.Cmd) {
	m = m.applyFocus()
	if m.totalWidth > 0 {
		m = m.recalcLayout()
		var refreshCmd tea.Cmd
		m.editor, refreshCmd = m.editor.RefreshImagesAfterLayoutChange()
		if refreshCmd != nil {
			cmds = append(cmds, refreshCmd)
		}
	}
	return m, tea.Batch(cmds...)
}

func (m Model) syncDirty() Model {
	if m.filePath == "" {
		return m
	}
	if m.editor.Revision() != m.cleanRev {
		m.opentabs = m.opentabs.MarkDirty(m.filePath)
	} else {
		m.opentabs = m.opentabs.MarkClean(m.filePath)
	}
	return m
}

func (m Model) finalize(cmds []tea.Cmd) (Model, tea.Cmd) {
	m = m.syncDirty()
	m = m.applyFocus()
	if m.totalWidth > 0 {
		m = m.recalcLayout()
	}
	return m, tea.Batch(cmds...)
}

func (m Model) Update(msg tea.Msg) (Model, tea.Cmd) {
	var cmds []tea.Cmd
	var cmd tea.Cmd

	// Always forward all messages to the dictation component (engine management).
	m.dict, cmd = m.dict.Update(msg)
	cmds = append(cmds, cmd)
	// Drain any pending edit from dictation and route to the focused target (D16).
	var s, e int
	var t string
	var hasPending bool
	m.dict, s, e, t, hasPending = m.dict.TakePendingEdit()
	if hasPending {
		switch m.focus {
		case paneCenter:
			prevCursors := m.editor.Cursors()
			m.editor, cmd = m.editor.ReplaceRange(s, e, t)
			cmds = append(cmds, cmd)
			var dictEdits []buffer.AppliedEdit
			m.editor, dictEdits = m.editor.DrainEdits()
			m = m.journalEdit("main", dictEdits, prevCursors, m.editor.Cursors(), &cmds)
		case paneChat:
			m.chat = m.chat.ApplyToPrompt(s, e, t)
		}
	}

	switch msg := msg.(type) {
	case tea.WindowSizeMsg:
		m.totalWidth, m.totalHeight = msg.Width, msg.Height
		m = m.recalcLayout()

	case tea.KeyPressMsg:
		// Priority 2: Save in-flight — consume all keys.
		if m.activeSave.InFlight {
			return m.finalize(cmds)
		}

		// Priority 2.5: Global undo/redo (intercept before any surface sees the key).
		switch {
		case key.Matches(msg, m.keys.Undo):
			var undoCmd tea.Cmd
			m, undoCmd = m.handleUndo()
			cmds = append(cmds, undoCmd)
			return m.finalize(cmds)
		case key.Matches(msg, m.keys.Redo):
			var redoCmd tea.Cmd
			m, redoCmd = m.handleRedo()
			cmds = append(cmds, redoCmd)
			return m.finalize(cmds)
		}

		// Priority 3: Global workspace keys.
		consumed := true
		switch {
		case key.Matches(msg, m.keys.SaveFile):
			if m.filePath != "" && !m.activeSave.InFlight {
				m, cmd = m.startSave()
				cmds = append(cmds, cmd)
			}

		case key.Matches(msg, m.keys.TabSwitch):
			var ok bool
			m, cmd, ok = m.maybeFinalizeTitle()
			cmds = append(cmds, cmd)
			if !ok {
				return m.finalize(cmds)
			}
			digit := msg.BaseCode
			if digit == 0 {
				digit = msg.Code
			}
			if digit >= '1' && digit <= '9' {
				idx := int(digit - '1')
				if path := m.opentabs.PathAt(idx); path != "" {
					m.opentabs = m.opentabs.SelectIndex(idx)
					m, cmd = m.requestOpenPath(path)
					cmds = append(cmds, cmd)
				}
			}

		case key.Matches(msg, m.keys.PinTab):
			var ok bool
			m, cmd, ok = m.maybeFinalizeTitle()
			cmds = append(cmds, cmd)
			if !ok {
				return m.finalize(cmds)
			}
			m.opentabs = m.opentabs.PinIndex(m.opentabs.Cursor())

		case key.Matches(msg, m.keys.FocusExplorer):
			var ok bool
			m, cmd, ok = m.maybeFinalizeTitle()
			cmds = append(cmds, cmd)
			if !ok {
				return m.finalize(cmds)
			}
			m.focus = paneTree
			m.leftVisible = true
			m = m.syncDictationAllowed()

		case key.Matches(msg, m.keys.FocusEditor):
			var ok bool
			m, cmd, ok = m.maybeFinalizeTitle()
			cmds = append(cmds, cmd)
			if !ok {
				return m.finalize(cmds)
			}
			m.focus = paneCenter
			m = m.syncDictationAllowed()

		case key.Matches(msg, m.keys.FocusChat):
			var ok bool
			m, cmd, ok = m.maybeFinalizeTitle()
			cmds = append(cmds, cmd)
			if !ok {
				return m.finalize(cmds)
			}
			if m.rightVisible && m.focus == paneChat {
				m.rightVisible = false
				m.focus = paneCenter
			} else {
				m.rightVisible = true
				m.focus = paneChat
			}
			m = m.syncDictationAllowed()

		case key.Matches(msg, m.keys.CreateNewFile):
			prevFocus := m.focus
			var ok bool
			var titleCmd tea.Cmd
			m, titleCmd, ok = m.maybeFinalizeTitle()
			cmds = append(cmds, titleCmd)
			if !ok {
				return m.finalize(cmds)
			}
			if m.filePath != "" || m.editor.Content() != "" {
				// titleCmd is non-nil only when the title pane was focused and
				// the user had edited the name: a RenameRequestMsg is in flight
				// and will create the file. Skip auto-save in that case.
				renameInFlight := prevFocus == paneTitle && titleCmd != nil
				m, cmd = m.CreateUntitled(!renameInFlight)
				cmds = append(cmds, cmd)
			}
			m.title = m.title.FocusAndSelectAll()
			m.focus = paneTitle
			m = m.syncDictationAllowed()

		case key.Matches(msg, m.keys.CloseFile):
			var ok bool
			m, cmd, ok = m.maybeFinalizeTitle()
			cmds = append(cmds, cmd)
			if !ok {
				return m.finalize(cmds)
			}
			m, cmd = m.requestCloseCurrent()
			cmds = append(cmds, cmd)

		case key.Matches(msg, m.keys.ZenMode):
			var ok bool
			m, cmd, ok = m.maybeFinalizeTitle()
			cmds = append(cmds, cmd)
			if !ok {
				return m.finalize(cmds)
			}
			m.leftVisible = !m.leftVisible
			if !m.leftVisible && m.focus.isLeft() {
				m.focus = paneCenter
			}
			m = m.syncDictationAllowed()

		default:
			consumed = false
		}

		if consumed {
			if key.Matches(msg, m.keys.ConfirmExitC) || key.Matches(msg, m.keys.ConfirmExitD) {
				m.footer, cmd = m.footer.Update(msg)
				cmds = append(cmds, cmd)
			}
			return m.finalize(cmds)
		}

		// Project focus before any key reaches a child, so the target pane is
		// focused when it receives the key regardless of how m.focus was set.
		m = m.applyFocus()

		// Priority 3b: D11 — Up at editor top transfers focus to title.
		if m.focus == paneCenter && msg.Code == tea.KeyUp && msg.Mod == 0 && m.editor.CursorAtTop() {
			m.focus = paneTitle
			m.title = m.title.FocusAtEnd() // cursor gesture; focus bools projected by finalize
			return m.finalize(cmds)        // consume Up; don't also move cursor
		}

		// Priority 4: Editor wants modal input — skip footer help toggle.
		if m.focus == paneCenter && m.editor.WantsModalInput() {
			prevCursors := m.editor.Cursors()
			m.editor, cmd = m.editor.Update(msg)
			cmds = append(cmds, cmd)
			var modalEdits []buffer.AppliedEdit
			m.editor, modalEdits = m.editor.DrainEdits()
			m = m.journalEdit("main", modalEdits, prevCursors, m.editor.Cursors(), &cmds)
			m = m.syncCursorToFooter()
			return m.finalize(cmds)
		}

		// Singular key routing — exactly one child receives each KeyPressMsg.
		switch m.focus {
		case paneTitle:
			prevCursors := m.title.Cursors()
			m.title, cmd = m.title.Update(msg)
			cmds = append(cmds, cmd)
			var titleEdits []buffer.AppliedEdit
			m.title, titleEdits = m.title.DrainEdits()
			m = m.journalEdit("title", titleEdits, prevCursors, m.title.Cursors(), &cmds)
		case paneCenter:
			if !m.dict.Enabled() {
				prevCursors := m.editor.Cursors()
				m.editor, cmd = m.editor.Update(msg)
				cmds = append(cmds, cmd)
				var editorEdits []buffer.AppliedEdit
				m.editor, editorEdits = m.editor.DrainEdits()
				m = m.journalEdit("main", editorEdits, prevCursors, m.editor.Cursors(), &cmds)
			}
		case paneChat:
			prevCursors := m.chat.Cursors()
			m.chat, cmd = m.chat.Update(msg)
			cmds = append(cmds, cmd)
			var chatEdits []buffer.AppliedEdit
			m.chat, chatEdits = m.chat.DrainEdits()
			m = m.journalEdit("chat", chatEdits, prevCursors, m.chat.Cursors(), &cmds)
		case paneTree:
			m.filetree, cmd = m.filetree.Update(msg)
			cmds = append(cmds, cmd)
		case paneTabs:
			m.opentabs, cmd = m.opentabs.Update(msg)
			cmds = append(cmds, cmd)
		}
		// Footer always gets keys for chord/help handling.
		m.footer, cmd = m.footer.Update(msg)
		cmds = append(cmds, cmd)

		m = m.syncCursorToFooter()
		return m.finalize(cmds)

	case title.FocusReturnMsg:
		// Title emits this on Down/Enter — return focus to editor (D11).
		// Focus bools are projected from m.focus by applyFocus.
		m.focus = paneCenter
		m = m.syncDictationAllowed()

	case title.RenameRequestMsg:
		if m.filePath == "" {
			// Untitled file gets a name — create on disk.
			dir := m.currentDir()
			newPath := filepath.Join(dir, msg.Name+".md")
			m.filePath = newPath
			m.breadcrumb = m.breadcrumb.SetPath(newPath)
			m.opentabs = m.opentabs.OpenFile(newPath)
			cmds = append(cmds, createFileCmd(newPath, m.editor.Content()))
		} else {
			// Existing file rename.
			dir := filepath.Dir(m.filePath)
			newPath := filepath.Join(dir, msg.Name+".md")
			cmds = append(cmds, fileRenameCmd(m.filePath, newPath))
		}

	case filetree.FileSelectedMsg:
		m, cmd = m.requestOpenPath(msg.Path)
		cmds = append(cmds, cmd)

	case filetree.DirSelectedMsg:
		cmds = append(cmds, loadDirCmd(msg.Path))

	case filetree.DirLoadedMsg:
		m.editor = m.editor.SetDir(msg.Root)
		m.breadcrumb = m.breadcrumb.SetDir(msg.Root)
		m, cmd = m.startWatch(msg.Root)
		cmds = append(cmds, cmd)

	case dirChangedMsg:
		dir := m.watchedDir
		cmds = append(cmds, reloadDirCmd(dir))

	case opentabs.TabSelectedMsg:
		m, cmd = m.requestOpenPath(msg.Path)
		cmds = append(cmds, cmd)

	case markdownedit.LinkClickedMsg:
		if msg.Path != "" {
			m, cmd = m.requestOpenPath(msg.Path)
			cmds = append(cmds, cmd)
		}

	case FileLoadedMsg:
		// Discard the empty untitled placeholder when transitioning to a real file.
		// If the buffer already has content the user hasn't saved, keep its tab.
		if m.filePath == "" && m.editor.Content() == "" {
			m.opentabs = m.opentabs.CloseFile("")
		}
		m.editor = m.editor.SetContent(string(msg.Content))
		m.filePath = msg.Path
		m.breadcrumb = m.breadcrumb.SetPath(msg.Path)
		m.opentabs = m.opentabs.OpenFile(msg.Path)
		m.opentabs = m.opentabs.MarkClean(msg.Path)
		m.cleanRev = m.editor.Revision()
		m.chat = m.chat.SetFileContext(msg.Path, string(msg.Content))
		// Ensure document row for this file.
		if msg.Path != "" && m.store != nil {
			docID, err := m.store.EnsureDocument(msg.Path)
			if err == nil {
				m.docID = docID
			}
		}
		// Update title from filename
		if msg.Path != "" {
			base := filepath.Base(msg.Path)
			if strings.HasSuffix(base, ".md") {
				base = base[:len(base)-3]
			}
			m.title = m.title.SetText(base)
		}

	case FileRenamedMsg:
		m.opentabs = m.opentabs.RenameFile(msg.OldPath, msg.NewPath)
		m.filePath = msg.NewPath
		m.breadcrumb = m.breadcrumb.SetPath(msg.NewPath)
		// Update title from new filename
		base := filepath.Base(msg.NewPath)
		if strings.HasSuffix(base, ".md") {
			base = base[:len(base)-3]
		}
		m.title = m.title.SetText(base)

	case FileRenameErrorMsg:
		m.footer, cmd = m.footer.Update(footer.ShowErrorMsg{Text: msg.Err.Error()})
		cmds = append(cmds, cmd)

	case fileCreatedMsg:
		if msg.err != nil {
			m.footer, cmd = m.footer.Update(footer.ShowErrorMsg{Text: msg.err.Error()})
			cmds = append(cmds, cmd)
		}

	case FileSavedMsg:
		if m.activeSave.InFlight && m.activeSave.RequestID == msg.RequestID {
			m.activeSave.InFlight = false
			m.cleanRev = m.editor.Revision()
			m.opentabs = m.opentabs.MarkClean(m.filePath)
			if m.pendingQuitAfterSave {
				m.pendingQuitAfterSave = false
				m.dict = m.dict.Disable()
				if m.store != nil {
					_ = m.store.Close()
				}
				return m, tea.Sequence(m.editor.DeleteAllImagesCmd(), tea.Quit)
			}
		}

	case FileSaveErrorMsg:
		if m.activeSave.InFlight && m.activeSave.RequestID == msg.RequestID {
			m.activeSave.InFlight = false
			m.err = msg.Err
		}

	case FileLoadErrorMsg:
		// Ignore (e.g., broken link click to missing file)

	case fileWatchReadError:
		m.err = fmt.Errorf("external change to %s: %w", msg.path, msg.err)

	case StoreReadyMsg:
		m.store = msg.Store
		if msg.Warning != "" {
			m.footer, cmd = m.footer.Update(footer.ShowErrorMsg{Text: msg.Warning})
			cmds = append(cmds, cmd)
		}
		// Ensure document row for current file (if any).
		if m.filePath != "" && m.store != nil {
			docID, err := m.store.EnsureDocument(m.filePath)
			if err == nil {
				m.docID = docID
			}
		}

	case pendingFlushMsg:
		if msg.gen == m.flushGen {
			// Autosave to disk if we have a path.
			if m.filePath != "" && !m.activeSave.InFlight {
				m, cmd = m.startSave()
				cmds = append(cmds, cmd)
			}
		}

	case ErrMsg:
		m.err = msg.Err

	case tea.MouseClickMsg:
		m.drag = dragNone

		if d, ok := m.dividerAtPoint(msg.X, msg.Y); ok {
			m.drag = d
			if d == dragLeft && !m.leftVisible {
				m.leftVisible = true
				m.leftPaneW = minLeftPaneW
			} else if d == dragRight && !m.rightVisible {
				m.rightVisible = true
				m.rightPaneW = minRightPaneW
			}
			return m.finalizeLayoutChange(cmds)
		}
		if newFocus, ok := m.paneAtPoint(msg.X, msg.Y); ok {
			if newFocus == paneTitle {
				// Clicking title area focuses the title (cursor to end);
				// focus bools are projected from m.focus by applyFocus.
				m.focus = paneTitle
				m.title = m.title.FocusAtEnd()
			} else {
				if m.focus == paneTitle {
					var finalizeCmd tea.Cmd
					var finalizeOk bool
					m, finalizeCmd, finalizeOk = m.maybeFinalizeTitle()
					cmds = append(cmds, finalizeCmd)
					if !finalizeOk {
						return m.finalize(cmds)
					}
				}
				m.focus = newFocus
			}
			m = m.syncDictationAllowed()
		}

	case tea.MouseMotionMsg:
		if m.drag == dragNone {
			break
		}
		if msg.Button != tea.MouseLeft {
			m.drag = dragNone
			return m.finalize(cmds)
		}
		switch m.drag {
		case dragLeft:
			newW := msg.X
			if newW < minLeftPaneW {
				m.leftVisible = false
				m.leftPaneW = defaultLeftPaneW
				m.drag = dragNone
				if m.focus.isLeft() {
					m.focus = paneCenter
					m = m.syncDictationAllowed()
				}
			} else {
				rightW := 0
				if m.rightVisible {
					rightW = m.rightPaneW
				}
				if max := m.totalWidth - rightW - minCenterW; newW > max {
					newW = max
				}
				m.leftPaneW = newW
				m.leftVisible = true
			}
		case dragRight:
			newW := m.totalWidth - msg.X
			if newW < minRightPaneW {
				m.rightVisible = false
				m.rightPaneW = defaultRightPaneW
				m.drag = dragNone
				if m.focus == paneChat {
					m.focus = paneCenter
					m = m.syncDictationAllowed()
				}
			} else {
				leftW := 0
				if m.leftVisible {
					leftW = m.leftPaneW
				}
				if max := m.totalWidth - leftW - minCenterW; newW > max {
					newW = max
				}
				m.rightPaneW = newW
				m.rightVisible = true
			}
		}
		return m.finalizeLayoutChange(cmds)

	case footer.ConfirmQuitMsg:
		if m.opentabs.HasDirty() {
			m.footer = m.footer.SetGuard(footer.GuardDirty, quitGuardOptions)
			return m, nil
		}
		m.dict = m.dict.Disable()
		if m.store != nil {
			_ = m.store.Close()
		}
		return m, tea.Sequence(m.editor.DeleteAllImagesCmd(), tea.Quit)

	case footer.DataLossGuardResponseMsg:
		switch msg.Response {
		case footer.DataLossSave:
			m.pendingQuitAfterSave = true
			m, cmd = m.startSave()
			cmds = append(cmds, cmd)
		case footer.DataLossDiscard:
			m.opentabs = m.opentabs.MarkClean(m.filePath)
			m.cleanRev = m.editor.Revision()
			m.dict = m.dict.Disable()
			if m.store != nil {
				_ = m.store.Close()
			}
			return m, tea.Sequence(m.editor.DeleteAllImagesCmd(), tea.Quit)
		case footer.DataLossCancel:
			// guard already cleared by footer; do nothing
		}

	case footer.DictationStartMsg:
		m.dict = m.dict.Enable(m.editor.CursorOffset())
		if m.focus == paneCenter {
			m.dict, cmd = m.dict.StartCmd()
			cmds = append(cmds, cmd)
		}

	case footer.DictationStopMsg:
		m.dict = m.dict.Disable()

	case dictcomp.DoneMsg:
		m.footer = m.footer.SetDictating(false)

	case dictengine.ErrorMsg:
		if msg.Fatal {
			m.footer = m.footer.SetDictating(false)
		}
	}

	// Forward non-key messages to all children (broadcast path).
	if _, isKey := msg.(tea.KeyPressMsg); !isKey {
		// Project focus before forwarding so children handle a focus-changing
		// message (e.g. a click that refocuses and positions the cursor) consistently.
		m = m.applyFocus()

		m.title, cmd = m.title.Update(msg)
		cmds = append(cmds, cmd)

		m.filetree, cmd = m.filetree.Update(msg)
		cmds = append(cmds, cmd)

		m.opentabs, cmd = m.opentabs.Update(msg)
		cmds = append(cmds, cmd)

		m.editor, cmd = m.editor.Update(msg)
		cmds = append(cmds, cmd)

		m.chat, cmd = m.chat.Update(msg)
		cmds = append(cmds, cmd)

		m.footer, cmd = m.footer.Update(msg)
		cmds = append(cmds, cmd)

		m = m.syncCursorToFooter()
	}

	return m.finalize(cmds)
}

func (m Model) View() tea.View {
	if m.totalWidth == 0 {
		return tea.NewView("")
	}

	contentH := m.totalHeight - m.footer.Height()
	if contentH < 0 {
		contentH = 0
	}
	leftW := 0
	if m.leftVisible {
		leftW = m.leftPaneW
	}
	rightW := 0
	if m.rightVisible {
		rightW = m.rightPaneW
	}
	centerW := m.totalWidth - leftW - rightW
	if centerW < 0 {
		centerW = 0
	}

	// Center pane: title + editor vertically composed
	centerContent := lipgloss.JoinVertical(lipgloss.Left, m.title.View(), m.editor.View())
	centerBlock := borderStyle(m.focus.isCenter(), m.styles).
		Width(centerW).Height(contentH).
		Render(centerContent)
	centerBlock = overlayBreadcrumb(centerBlock, m.breadcrumb.View(), m.focus.isCenter(), m.styles)

	var chatBlock string
	if m.rightVisible {
		chatBlock = borderStyle(m.focus == paneChat, m.styles).
			Width(rightW).Height(contentH).
			Render(m.chat.View())
	}

	var body string
	switch {
	case m.leftVisible && m.rightVisible:
		leftContent := lipgloss.JoinVertical(lipgloss.Left,
			m.filetree.View(),
			m.opentabs.View(),
		)
		leftBlock := borderStyle(m.focus.isLeft(), m.styles).
			Width(leftW).Height(contentH).
			Render(leftContent)
		body = lipgloss.JoinHorizontal(lipgloss.Top, leftBlock, centerBlock, chatBlock)

	case m.leftVisible && !m.rightVisible:
		leftContent := lipgloss.JoinVertical(lipgloss.Left,
			m.filetree.View(),
			m.opentabs.View(),
		)
		leftBlock := borderStyle(m.focus.isLeft(), m.styles).
			Width(leftW).Height(contentH).
			Render(leftContent)
		body = lipgloss.JoinHorizontal(lipgloss.Top, leftBlock, centerBlock)

	case !m.leftVisible && m.rightVisible:
		body = lipgloss.JoinHorizontal(lipgloss.Top, centerBlock, chatBlock)

	default: // zen mode
		body = centerBlock
	}

	if m.err != nil {
		errLine := m.styles.Error.Render("error: " + m.err.Error())
		frame := lipgloss.JoinVertical(lipgloss.Left, errLine, body, m.footer.View())
		return tea.NewView(frame)
	}
	frame := lipgloss.JoinVertical(lipgloss.Left, body, m.footer.View())
	return tea.NewView(frame)
}

func overlayBreadcrumb(block, crumb string, active bool, st styles.Styles) string {
	if crumb == "" {
		return block
	}

	lines := strings.Split(block, "\n")
	if len(lines) < 2 {
		return block
	}

	lastIdx := len(lines) - 1
	bottomLine := lines[lastIdx]
	borderW := lipgloss.Width(bottomLine)

	bcWidth := lipgloss.Width(crumb)
	minOverhead := 7
	if bcWidth+minOverhead > borderW {
		return block
	}

	borderColor := st.InactiveBorder.GetBorderTopForeground()
	if active {
		borderColor = st.ActiveBorder.GetBorderTopForeground()
	}
	bStyle := lipgloss.NewStyle().Foreground(borderColor)

	rightPad := bStyle.Render("──╯")
	leftCorner := bStyle.Render("╰")
	content := " " + crumb + " "
	contentWidth := lipgloss.Width(content) + lipgloss.Width(rightPad) + lipgloss.Width(leftCorner)
	dashCount := borderW - contentWidth
	if dashCount < 0 {
		dashCount = 0
	}
	dashFill := bStyle.Render(strings.Repeat("─", dashCount))

	lines[lastIdx] = leftCorner + dashFill + content + rightPad
	return strings.Join(lines, "\n")
}

func borderStyle(active bool, st styles.Styles) lipgloss.Style {
	if active {
		return st.ActiveBorder
	}
	return st.InactiveBorder
}

func loadDirCmd(dir string) tea.Cmd {
	return func() tea.Msg {
		entries, err := readDirEntries(dir)
		if err != nil {
			return ErrMsg{Err: err}
		}
		return filetree.DirLoadedMsg{Root: dir, Entries: entries}
	}
}

func reloadDirCmd(dir string) tea.Cmd {
	return func() tea.Msg {
		entries, err := readDirEntries(dir)
		if err != nil {
			return ErrMsg{Err: err}
		}
		return filetree.DirReloadedMsg{Root: dir, Entries: entries}
	}
}

func readDirEntries(dir string) ([]filetree.Entry, error) {
	des, err := os.ReadDir(dir)
	if err != nil {
		return nil, fmt.Errorf("load dir %q: %w", dir, err)
	}
	entries := make([]filetree.Entry, 0, len(des)+1)
	if dir == "/" {
		entries = append(entries, filetree.Entry{
			Name:  "/",
			Path:  "/",
			IsDir: true,
		})
	} else {
		entries = append(entries, filetree.Entry{
			Name:  "..",
			Path:  filepath.Dir(dir),
			IsDir: true,
		})
	}
	for _, de := range des {
		entries = append(entries, filetree.Entry{
			Name:  de.Name(),
			Path:  filepath.Join(dir, de.Name()),
			IsDir: de.IsDir(),
		})
	}
	sort.Slice(entries, func(i, j int) bool {
		a, b := entries[i], entries[j]
		if a.Name == ".." {
			return true
		}
		if b.Name == ".." {
			return false
		}
		if a.IsDir != b.IsDir {
			return a.IsDir
		}
		return strings.ToLower(a.Name) < strings.ToLower(b.Name)
	})
	return entries, nil
}

func (m Model) startWatch(dir string) (Model, tea.Cmd) {
	if m.cancelWatch != nil {
		m.cancelWatch()
	}
	ctx, cancel := context.WithCancel(context.Background())
	m.cancelWatch = cancel
	m.watchedDir = dir
	return m, watchDirCmd(ctx, dir)
}


type fileCreatedMsg struct {
	path    string
	content string
	err     error
}

func createFileCmd(path, content string) tea.Cmd {
	return func() tea.Msg {
		dir := filepath.Dir(path)
		if err := os.MkdirAll(dir, 0755); err != nil {
			return fileCreatedMsg{path: path, err: fmt.Errorf("mkdir %q: %w", dir, err)}
		}
		if err := os.WriteFile(path, []byte(content), 0644); err != nil {
			return fileCreatedMsg{path: path, err: fmt.Errorf("create %q: %w", path, err)}
		}
		return fileCreatedMsg{path: path, content: content}
	}
}

const invalidFileNameChars = "/\\:*?\"<>|\x00"

func validateFileName(name string) error {
	if name == "" {
		return fmt.Errorf("name must not be empty")
	}
	for _, r := range name {
		for _, bad := range invalidFileNameChars {
			if r == bad {
				return fmt.Errorf("name contains invalid character %q", r)
			}
		}
		if r < 32 {
			return fmt.Errorf("name contains control character")
		}
	}
	return nil
}

// maybeFinalizeTitle validates and commits the title when focus is leaving paneTitle.
// Returns (model, cmd, ok) — if ok is false, focus change is blocked (validation failed).
func (m Model) maybeFinalizeTitle() (Model, tea.Cmd, bool) {
	if m.focus != paneTitle {
		return m, nil, true
	}
	if err := validateFileName(m.title.Text()); err != nil {
		var errCmd tea.Cmd
		m.footer, errCmd = m.footer.Update(footer.ShowErrorMsg{Text: "invalid name: " + err.Error()})
		// Focus stays on the title: m.focus is unchanged here and callers return
		// without advancing it, so applyFocus keeps the title focused.
		return m, errCmd, false
	}
	var renameCmd tea.Cmd
	m.title, renameCmd = m.title.Commit()
	return m, renameCmd, true
}

// nextUntitled returns the first available "Untitled N" name in dir.
// skip, if non-empty, is excluded from the search — used when the caller has
// already reserved that name for an in-memory buffer not yet written to disk.
func nextUntitled(dir, skip string) string {
	for n := 1; ; n++ {
		name := fmt.Sprintf("Untitled %d", n)
		if skip != "" && name == skip {
			continue
		}
		info, err := os.Stat(filepath.Join(dir, name+".md"))
		if err != nil || info.Size() == 0 {
			return name
		}
	}
}

// CreateUntitled opens a new untitled buffer in the current filetree directory.
// When preserveCurrentUntitled is true and the current buffer is an unsaved
// untitled file with content, the content is written to disk under its current
// title so it is not lost.
func (m Model) CreateUntitled(preserveCurrentUntitled bool) (Model, tea.Cmd) {
	dir := m.currentDir()
	var cmds []tea.Cmd

	skipName := ""
	if preserveCurrentUntitled && m.filePath == "" && m.editor.Content() != "" {
		skipName = m.title.Text()
		currentPath := filepath.Join(dir, skipName+".md")
		cmds = append(cmds, createFileCmd(currentPath, m.editor.Content()))
		m.opentabs = m.opentabs.RenameFile("", currentPath)
	}

	name := nextUntitled(dir, skipName)
	m.editor = m.editor.SetContent("")
	m.filePath = ""
	m.docID = 0
	m.title = m.title.SetText(name)
	m.breadcrumb = m.breadcrumb.SetPath("")
	m.opentabs = m.opentabs.OpenFile("")
	m.opentabs = m.opentabs.SetTabName("", name)
	m.focus = paneCenter
	return m, tea.Batch(cmds...)
}
