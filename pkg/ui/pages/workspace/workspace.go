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
	"github.com/fsnotify/fsnotify"

	dictengine "rune/pkg/dictation"
	"rune/pkg/editor/keybind"
	"rune/pkg/merge"
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

type pendingDirtyKind int

const (
	pendingSwitchFile pendingDirtyKind = iota
	pendingCloseFile
)

var dirtyGuardOptions = []footer.GuardOption{
	{Key: 's', Response: footer.DataLossSave},
	{Key: 'd', Response: footer.DataLossDiscard},
}

var mergeGuardOptions = []footer.GuardOption{
	{Key: 'y', Response: footer.DataLossMergeAccept},
	{Key: 'n', Response: footer.DataLossMergeReject},
}

type pendingDirtyAction struct {
	kind     pendingDirtyKind
	path     string // target path for switch; current path for close
	nextPath string // closeFile only: file to load after close
}

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
	pending                 *pendingDirtyAction

	// Dictation (D16)
	dict dictcomp.Model

	// Directory watching
	watchedDir  string
	cancelWatch context.CancelFunc

	// File ownership (D12)
	filePath   string
	activeSave SaveIdentity

	// Dirty tracking (D13)
	lastRev     uint64
	origContent []byte

	// 3-way merge state
	pendingMergeContent []byte

	// File watching
	watchedFilePath string
	cancelFileWatch context.CancelFunc
}

func New(keys keymap.Bindings, st styles.Styles, reg command.Registry, resolver keybind.Resolver, caps terminal.TermCaps) Model {
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
	}
	m = m.syncDictationAllowed()
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

func (m Model) isDirty() bool {
	return m.editor.Content() != string(m.origContent)
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
	return tea.Batch(
		m.filetree.Init(),
		m.opentabs.Init(),
		m.editor.Init(),
		m.footer.Init(),
		m.chat.Init(),
		m.dict.Init(),
		loadDirCmd(".", "."),
	)
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
	if m.isDirty() {
		m.pending = &pendingDirtyAction{kind: pendingSwitchFile, path: path}
		m.footer = m.footer.SetGuard(footer.GuardDirty, dirtyGuardOptions)
		return m, nil
	}
	var cmds []tea.Cmd
	var watchCmd tea.Cmd
	if path != "" {
		m, watchCmd = m.startFileWatch(path)
		cmds = append(cmds, watchCmd)
	}
	cmds = append(cmds, loadFileCmd(context.Background(), path))
	return m, tea.Batch(cmds...)
}

func (m Model) requestCloseCurrent() (Model, tea.Cmd) {
	if m.filePath == "" {
		return m, nil
	}
	nextPath := m.opentabs.NextPath(m.filePath)
	if m.isDirty() {
		m.pending = &pendingDirtyAction{
			kind:     pendingCloseFile,
			path:     m.filePath,
			nextPath: nextPath,
		}
		m.footer = m.footer.SetGuard(footer.GuardDirty, dirtyGuardOptions)
		m = m.stopFileWatch()
		return m, nil
	}
	m = m.stopFileWatch()
	return m.executeClose(m.filePath, nextPath)
}

func (m Model) executeClose(closePath, nextPath string) (Model, tea.Cmd) {
	var cmds []tea.Cmd
	m.opentabs = m.opentabs.CloseFile(closePath)
	if nextPath != "" {
		var watchCmd tea.Cmd
		m, watchCmd = m.startFileWatch(nextPath)
		cmds = append(cmds, watchCmd)
		cmds = append(cmds, loadFileCmd(context.Background(), nextPath))
	} else {
		// Last tab closed — reset to a fresh untitled buffer
		var createCmd tea.Cmd
		m, createCmd = m.CreateUntitled()
		cmds = append(cmds, createCmd)
	}
	return m, tea.Batch(cmds...)
}

func (m Model) handleDataLossGuardResponse(resp footer.DataLossGuardResponse) (Model, tea.Cmd) {
	var cmd tea.Cmd
	// Handle merge responses (no pending action needed).
	switch resp {
	case footer.DataLossMergeAccept:
		ours := []byte(m.editor.Content())
		theirs := m.pendingMergeContent
		m.pendingMergeContent = nil
		m.footer = m.footer.SetGuard(0, nil)

		opts := merge.DefaultOptions()
		opts.OursLabel = "ours"
		opts.TheirsLabel = "theirs"
		result, err := merge.Merge(m.origContent, ours, theirs, opts)
		if err != nil {
			m.footer, cmd = m.footer.Update(footer.ShowErrorMsg{Text: err.Error()})
			return m, cmd
		}
		m.editor, cmd = m.editor.ApplyMergeResult(string(result.Output))
		m.origContent = result.Output // merged result becomes new ancestor
		// Emit merged notification for opentabs dirty state
		path := m.filePath
		conflicted := result.Conflicted
		if m.isDirty() {
			m.opentabs = m.opentabs.MarkDirty(path)
		}
		cmds := []tea.Cmd{cmd}
		if conflicted {
			// Just mark dirty; no special message needed
			m.opentabs = m.opentabs.MarkDirty(path)
		}
		return m, tea.Batch(cmds...)

	case footer.DataLossMergeReject:
		diskContent := m.pendingMergeContent // what the disk now holds
		m.pendingMergeContent = nil
		m.footer = m.footer.SetGuard(0, nil)
		// Disk content is the new merge ancestor (D13 disk-sync point: merge-reject).
		m.origContent = diskContent
		return m, nil
	}

	// Handle dirty guard responses (require pending action).
	if m.pending == nil {
		return m, nil
	}
	switch resp {
	case footer.DataLossCancel:
		m.pending = nil
		return m, nil

	case footer.DataLossDiscard:
		m.opentabs = m.opentabs.MarkClean(m.filePath)
		switch m.pending.kind {
		case pendingSwitchFile:
			path := m.pending.path
			m.pending = nil
			var watchCmd tea.Cmd
			m, watchCmd = m.startFileWatch(path)
			return m, tea.Batch(watchCmd, loadFileCmd(context.Background(), path))
		case pendingCloseFile:
			closePath := m.pending.path
			nextPath := m.pending.nextPath
			m.pending = nil
			return m.executeClose(closePath, nextPath)
		}

	case footer.DataLossSave:
		var saveCmd tea.Cmd
		m, saveCmd = m.startSave()
		return m, saveCmd
	}
	return m, nil
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

// recomputeDirty updates opentabs dirty marker if the buffer revision changed (D13).
func (m Model) recomputeDirty(prevRev uint64) Model {
	if m.editor.Revision() == prevRev {
		return m
	}
	m.lastRev = m.editor.Revision()
	if m.isDirty() {
		m.opentabs = m.opentabs.MarkDirty(m.filePath)
		m.footer = m.footer.SetDirty(true)
		// First edit on untitled file → create on disk
		if m.filePath == "" && m.editor.Content() != "" {
			dir := m.currentDir()
			name := m.title.Text()
			newPath := filepath.Join(dir, name+".md")
			m.filePath = newPath
			m.breadcrumb = m.breadcrumb.SetPath(newPath)
			m.opentabs = m.opentabs.OpenFile(newPath)
		}
	} else {
		m.opentabs = m.opentabs.MarkClean(m.filePath)
		m.footer = m.footer.SetDirty(false)
	}
	return m
}

func (m Model) finalizeLayoutChange(cmds []tea.Cmd) (Model, tea.Cmd) {
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

func (m Model) finalize(cmds []tea.Cmd) (Model, tea.Cmd) {
	if m.totalWidth > 0 {
		m = m.recalcLayout()
	}
	return m, tea.Batch(cmds...)
}

func (m Model) Update(msg tea.Msg) (Model, tea.Cmd) {
	var cmds []tea.Cmd
	var cmd tea.Cmd

	// Always forward all messages to the dictation component (engine management).
	prevRev := m.editor.Revision()
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
			m.editor, cmd = m.editor.ReplaceRange(s, e, t)
			cmds = append(cmds, cmd)
			prevRev = m.editor.Revision() // update so recomputeDirty sees the new rev
		case paneChat:
			m.chat = m.chat.ApplyToPrompt(s, e, t)
		}
	}

	switch msg := msg.(type) {
	case tea.WindowSizeMsg:
		m.totalWidth, m.totalHeight = msg.Width, msg.Height
		m = m.recalcLayout()

	case tea.KeyPressMsg:
		// Priority 1: Dirty guard — footer consumes all keys.
		if m.footer.InGuard() {
			m.footer, cmd = m.footer.Update(msg)
			cmds = append(cmds, cmd)
			return m.finalize(cmds)
		}

		// Priority 2: Save in-flight — consume all keys.
		if m.activeSave.InFlight {
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

		// Priority 3b: D11 — Up at editor top transfers focus to title.
		if m.focus == paneCenter && msg.Code == tea.KeyUp && msg.Mod == 0 && m.editor.CursorAtTop() {
			m.focus = paneTitle
			m.editor = m.editor.SetFocused(false)
			m.title = m.title.SetFocused(true)
			return m.finalize(cmds) // consume Up; don't also move cursor
		}

		// Priority 4: Editor wants modal input — skip footer help toggle.
		if m.focus == paneCenter && m.editor.WantsModalInput() {
			m.editor = m.editor.SetFocused(true)
			m.editor, cmd = m.editor.Update(msg)
			cmds = append(cmds, cmd)
			m = m.recomputeDirty(prevRev)
			m = m.syncCursorToFooter()
			return m.finalize(cmds)
		}

		// Singular key routing — exactly one child receives each KeyPressMsg.
		switch m.focus {
		case paneTitle:
			m.title, cmd = m.title.Update(msg)
			cmds = append(cmds, cmd)
		case paneCenter:
			m.editor = m.editor.SetFocused(true)
			if !m.dict.Enabled() {
				m.editor, cmd = m.editor.Update(msg)
				cmds = append(cmds, cmd)
				m = m.recomputeDirty(prevRev)
			}
		case paneChat:
			m.chat, cmd = m.chat.Update(msg)
			cmds = append(cmds, cmd)
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
		m.focus = paneCenter
		m.title = m.title.SetFocused(false)
		m.editor = m.editor.SetFocused(true)
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
		cmds = append(cmds, loadDirCmd(msg.Path, "."))

	case filetree.DirLoadedMsg:
		m.editor = m.editor.SetDir(msg.Root)
		m.breadcrumb = m.breadcrumb.SetDir(msg.Root)
		m, cmd = m.startWatch(msg.Root)
		cmds = append(cmds, cmd)

	case dirChangedMsg:
		dir := m.watchedDir
		cmds = append(cmds, reloadDirCmd(dir, "."))

	case opentabs.TabSelectedMsg:
		m, cmd = m.requestOpenPath(msg.Path)
		cmds = append(cmds, cmd)

	case markdownedit.LinkClickedMsg:
		if msg.Path != "" {
			m, cmd = m.requestOpenPath(msg.Path)
			cmds = append(cmds, cmd)
		}

	case FileLoadedMsg:
		m.editor = m.editor.SetContent(string(msg.Content))
		m.filePath = msg.Path
		m.breadcrumb = m.breadcrumb.SetPath(msg.Path)
		m.origContent = msg.Content
		m.lastRev = m.editor.Revision()
		m.footer = m.footer.SetDirty(false)
		m.opentabs = m.opentabs.OpenFile(msg.Path)
		m.chat = m.chat.SetFileContext(msg.Path, string(msg.Content))
		// Start (or restart) the per-file watcher now that we have confirmed content.
		if msg.Path != "" {
			var watchCmd tea.Cmd
			m, watchCmd = m.startFileWatch(msg.Path)
			cmds = append(cmds, watchCmd)
		}
		// Update title from filename
		if msg.Path != "" {
			base := filepath.Base(msg.Path)
			if strings.HasSuffix(base, ".md") {
				base = base[:len(base)-3]
			}
			m.title = m.title.SetText(base)
		}

	case FileChangedOnDiskMsg:
		if m.filePath == "" || m.filePath != msg.Path {
			return m.finalize(cmds)
		}
		if string(msg.NewContent) == m.editor.Content() {
			m.origContent = msg.NewContent
			return m.finalize(cmds)
		}
		m.pendingMergeContent = msg.NewContent
		m.footer = m.footer.SetGuard(footer.GuardMerge, mergeGuardOptions)

	case FileMergedMsg:
		if m.isDirty() {
			m.opentabs = m.opentabs.MarkDirty(msg.Path)
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
		} else {
			m.origContent = []byte(msg.content)
		}

	case FileSavedMsg:
		if m.activeSave.InFlight && m.activeSave.RequestID == msg.RequestID {
			m.activeSave.InFlight = false
			m.origContent = m.activeSave.SavedContent // bytes written, not Content() — D13
			dirty := m.isDirty()
			if dirty {
				m.opentabs = m.opentabs.MarkDirty(m.filePath)
			} else {
				m.opentabs = m.opentabs.MarkClean(m.filePath)
			}
			m.footer = m.footer.SetDirty(dirty)
			// Continue any pending close/switch action
			if m.pending != nil {
				switch m.pending.kind {
				case pendingSwitchFile:
					path := m.pending.path
					m.pending = nil
					var watchCmd tea.Cmd
					m, watchCmd = m.startFileWatch(path)
					cmds = append(cmds, watchCmd, loadFileCmd(context.Background(), path))
				case pendingCloseFile:
					closePath := m.pending.path
					nextPath := m.pending.nextPath
					m.pending = nil
					m, cmd = m.executeClose(closePath, nextPath)
					cmds = append(cmds, cmd)
				}
			}
		}

	case FileSaveErrorMsg:
		if m.activeSave.InFlight && m.activeSave.RequestID == msg.RequestID {
			m.activeSave.InFlight = false
			m.err = msg.Err
			if m.pending != nil {
				m.pending = nil
			}
		}

	case FileLoadErrorMsg:
		// Ignore (e.g., broken link click to missing file)

	case fileWatchReadError:
		m.err = fmt.Errorf("external change to %s: %w", msg.path, msg.err)

	case footer.DataLossGuardResponseMsg:
		m, cmd = m.handleDataLossGuardResponse(msg.Response)
		cmds = append(cmds, cmd)

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
				// Clicking title area focuses the title.
				m.focus = paneTitle
				m.title = m.title.SetFocused(true)
				m.editor = m.editor.SetFocused(false)
			} else {
				if m.focus == paneTitle {
					var finalizeCmd tea.Cmd
					var finalizeOk bool
					m, finalizeCmd, finalizeOk = m.maybeFinalizeTitle()
					cmds = append(cmds, finalizeCmd)
					if !finalizeOk {
						return m.finalize(cmds)
					}
					m.title = m.title.SetFocused(false)
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
		m.dict = m.dict.Disable()
		return m, tea.Sequence(m.editor.DeleteAllImagesCmd(), tea.Quit)

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
		m.title, cmd = m.title.Update(msg)
		cmds = append(cmds, cmd)

		m.filetree = m.filetree.SetFocused(m.focus == paneTree)
		m.filetree, cmd = m.filetree.Update(msg)
		cmds = append(cmds, cmd)

		m.opentabs = m.opentabs.SetFocused(m.focus == paneTabs)
		m.opentabs, cmd = m.opentabs.Update(msg)
		cmds = append(cmds, cmd)

		m.editor = m.editor.SetFocused(m.focus == paneCenter)
		m.editor, cmd = m.editor.Update(msg)
		cmds = append(cmds, cmd)
		m = m.recomputeDirty(prevRev)

		m.chat = m.chat.SetFocused(m.focus == paneChat)
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

func loadDirCmd(dir string, initialRoot string) tea.Cmd {
	return func() tea.Msg {
		entries, err := readDirEntries(dir, initialRoot)
		if err != nil {
			return ErrMsg{Err: err}
		}
		return filetree.DirLoadedMsg{Root: dir, Entries: entries}
	}
}

func reloadDirCmd(dir string, initialRoot string) tea.Cmd {
	return func() tea.Msg {
		entries, err := readDirEntries(dir, initialRoot)
		if err != nil {
			return ErrMsg{Err: err}
		}
		return filetree.DirReloadedMsg{Root: dir, Entries: entries}
	}
}

func readDirEntries(dir string, initialRoot string) ([]filetree.Entry, error) {
	des, err := os.ReadDir(dir)
	if err != nil {
		return nil, fmt.Errorf("load dir %q: %w", dir, err)
	}
	entries := make([]filetree.Entry, 0, len(des)+1)
	if dir != initialRoot && dir != "." {
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

func watchDirCmd(ctx context.Context, dir string) tea.Cmd {
	return func() tea.Msg {
		watcher, err := fsnotify.NewWatcher()
		if err != nil {
			return nil // fire-and-forget: watcher creation failed
		}
		defer watcher.Close()

		if err := watcher.Add(dir); err != nil {
			return nil // fire-and-forget: dir may have been removed
		}

		for {
			select {
			case <-ctx.Done():
				return nil
			case event, ok := <-watcher.Events:
				if !ok {
					return nil
				}
				if event.Has(fsnotify.Create) || event.Has(fsnotify.Remove) || event.Has(fsnotify.Rename) {
					timer := time.NewTimer(50 * time.Millisecond)
				drain:
					for {
						select {
						case <-ctx.Done():
							timer.Stop()
							return nil
						case <-timer.C:
							break drain
						case _, ok := <-watcher.Events:
							if !ok {
								break drain
							}
							timer.Reset(50 * time.Millisecond)
						}
					}
					return dirChangedMsg{}
				}
			case _, ok := <-watcher.Errors:
				if !ok {
					return nil
				}
				return nil // fire-and-forget: watcher error
			}
		}
	}
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

func watchFileCmd(ctx context.Context, path string) tea.Cmd {
	return func() tea.Msg {
		watcher, err := fsnotify.NewWatcher()
		if err != nil {
			return nil // fire-and-forget: watcher creation failed
		}
		defer watcher.Close()

		if err := watcher.Add(path); err != nil {
			return nil // fire-and-forget: file may have been removed
		}

		for {
			select {
			case <-ctx.Done():
				return nil
			case event, ok := <-watcher.Events:
				if !ok {
					return nil
				}
				if event.Has(fsnotify.Write) {
					b, err := os.ReadFile(path)
					if err != nil {
						return fileWatchReadError{path: path, err: fmt.Errorf("read %q: %w", path, err)}
					}
					return FileChangedOnDiskMsg{Path: path, NewContent: b}
				}
			case _, ok := <-watcher.Errors:
				if !ok {
					return nil
				}
				return nil // fire-and-forget: watcher error
			}
		}
	}
}

func (m Model) startFileWatch(path string) (Model, tea.Cmd) {
	if m.cancelFileWatch != nil {
		m.cancelFileWatch()
	}
	ctx, cancel := context.WithCancel(context.Background())
	m.cancelFileWatch = cancel
	m.watchedFilePath = path
	return m, watchFileCmd(ctx, path)
}

func (m Model) stopFileWatch() Model {
	if m.cancelFileWatch != nil {
		m.cancelFileWatch()
		m.cancelFileWatch = nil
		m.watchedFilePath = ""
	}
	return m
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
		m.title = m.title.SetFocused(true)
		return m, errCmd, false
	}
	var renameCmd tea.Cmd
	m.title, renameCmd = m.title.Commit()
	return m, renameCmd, true
}

func nextUntitled(dir string) string {
	for n := 1; ; n++ {
		name := fmt.Sprintf("Untitled %d", n)
		info, err := os.Stat(filepath.Join(dir, name+".md"))
		if err != nil || info.Size() == 0 {
			return name
		}
	}
}

// CreateUntitled opens a new untitled buffer in the current filetree directory.
func (m Model) CreateUntitled() (Model, tea.Cmd) {
	dir := m.currentDir()
	name := nextUntitled(dir)
	m.editor = m.editor.SetContent("")
	m.filePath = ""
	m.origContent = nil
	m.lastRev = m.editor.Revision()
	m.title = m.title.SetText(name)
	m.breadcrumb = m.breadcrumb.SetPath("")
	m.opentabs = m.opentabs.OpenFile("")
	m.focus = paneCenter
	return m, nil
}
