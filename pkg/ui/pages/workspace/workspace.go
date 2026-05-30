package workspace

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"

	"charm.land/bubbles/v2/key"
	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"

	"rune/pkg/command"
	"rune/pkg/dictation"
	"rune/pkg/editor/keybind"
	"rune/pkg/inputlang"
	"rune/pkg/terminal"
	"rune/pkg/ui/components/chat"
	"rune/pkg/ui/components/editor"
	"rune/pkg/ui/components/filetree"
	"rune/pkg/ui/components/footer"
	"rune/pkg/ui/components/opentabs"
	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"
	"rune/pkg/whisper"
)

type pane int

const (
	paneTree pane = iota
	paneTabs
	paneCenter
	paneChat
)

func (p pane) isLeft() bool { return p == paneTree || p == paneTabs }

// ErrMsg signals an I/O error to the workspace page.
type ErrMsg struct{ Err error }

const defaultLeftPaneW = 22
const defaultRightPaneW = 38

type dragState int

const (
	dragNone dragState = iota
	dragLeft
	dragRight
)

// Width-constraint constants for mouse resizing.
// Below the per-pane minimum, the pane hides; the center pane has a hard
// floor so left/right drags cannot squeeze it to nothing.
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

type pendingDirtyAction struct {
	kind          pendingDirtyKind
	path          string // target path for switch; current path for close
	nextPath      string // closeFile only: file to load after close
	saveInFlight  bool
	saveRequestID string
}

type Model struct {
	totalWidth, totalHeight int
	filetree                filetree.Model
	opentabs                opentabs.Model
	editor                  editor.Model
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
	dictCancel              context.CancelFunc // nil when not dictating (§6.3)
	dictCh                  <-chan tea.Msg     // nil when idle
}

func New(keys keymap.Bindings, st styles.Styles, reg command.Registry, resolver keybind.Resolver, caps terminal.TermCaps) Model {
	m := Model{
		filetree:     filetree.New(keys, st),
		opentabs:     opentabs.New(keys, st),
		editor:       editor.New(keys, st, reg, resolver, caps),
		footer:       footer.New(keys, st).SetHelp(keys.HelpText()),
		chat:         chat.New(keys, st),
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

	otH := m.opentabs.Height()
	ftH := innerH - otH
	if ftH < 4 {
		ftH = 4
	}
	m.filetree = m.filetree.SetSize(innerLeftW, ftH)
	m.opentabs = m.opentabs.SetSize(innerLeftW, otH)
	m.editor = m.editor.SetSize(innerCenterW, innerH)
	m.chat = m.chat.SetSize(innerRightW, innerH)
	m.footer = m.footer.SetSize(m.totalWidth, m.footer.Height())
	m.filetree = m.filetree.SetOffset(1, 1)
	m.editor = m.editor.SetOffset(leftW+1, 1)
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
	return paneCenter, true
}

func (m Model) dividerAtPoint(x, y int) (dragState, bool) {
	contentH := m.totalHeight - m.footer.Height()
	if y < 0 || y >= contentH {
		return dragNone, false
	}

	// Left divider:
	//   visible: 2-column grab zone straddling the left/center border.
	//   hidden:  single column at the editor's left border only (x=0).
	if m.leftVisible {
		if x == m.leftPaneW-1 || x == m.leftPaneW {
			return dragLeft, true
		}
	} else {
		if x == 0 {
			return dragLeft, true
		}
	}

	// Right divider:
	//   visible: 2-column grab zone straddling the center/right border.
	//   hidden:  single column at the editor's right border only.
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
	// Capture the working directory for wiki link resolution.
	cwd, _ := os.Getwd()
	m.editor = m.editor.SetCWD(cwd)
	return tea.Batch(
		m.filetree.Init(),
		m.opentabs.Init(),
		m.editor.Init(),
		m.footer.Init(),
		m.chat.Init(),
		loadDirCmd(".", "."),
	)
}

func (m Model) requestOpenPath(path string) (Model, tea.Cmd) {
	if path == m.editor.FilePath() {
		return m, nil
	}
	if m.editor.IsDirty() {
		m.pending = &pendingDirtyAction{kind: pendingSwitchFile, path: path}
		m.footer = m.footer.SetDirtyGuard(true)
		return m, nil
	}
	return m, editor.LoadFileCmd(path)
}

func (m Model) requestCloseCurrent() (Model, tea.Cmd) {
	currentPath := m.editor.FilePath()
	if currentPath == "" {
		return m, nil
	}
	nextPath := m.opentabs.NextPath(currentPath)
	if m.editor.IsDirty() {
		m.pending = &pendingDirtyAction{
			kind:     pendingCloseFile,
			path:     currentPath,
			nextPath: nextPath,
		}
		m.footer = m.footer.SetDirtyGuard(true)
		return m, nil
	}
	return m.executeClose(currentPath, nextPath)
}

func (m Model) executeClose(closePath, nextPath string) (Model, tea.Cmd) {
	var cmds []tea.Cmd
	var cmd tea.Cmd
	m.opentabs = m.opentabs.CloseFile(closePath)
	m.editor, cmd = m.editor.Update(editor.FileClosedMsg{Path: closePath})
	cmds = append(cmds, cmd)
	if nextPath != "" {
		cmds = append(cmds, editor.LoadFileCmd(nextPath))
	}
	return m, tea.Batch(cmds...)
}

func (m Model) handleDirtyGuardResponse(resp footer.DirtyGuardResponse) (Model, tea.Cmd) {
	if m.pending == nil {
		return m, nil
	}
	switch resp {
	case footer.DirtyGuardCancel:
		m.pending = nil
		return m, nil

	case footer.DirtyGuardDiscard:
		m.opentabs = m.opentabs.MarkClean(m.editor.FilePath())
		switch m.pending.kind {
		case pendingSwitchFile:
			path := m.pending.path
			m.pending = nil
			return m, editor.LoadFileCmd(path)
		case pendingCloseFile:
			closePath := m.pending.path
			nextPath := m.pending.nextPath
			m.pending = nil
			return m.executeClose(closePath, nextPath)
		}

	case footer.DirtyGuardSave:
		var saveID editor.SaveIdentity
		var cmd tea.Cmd
		m.editor, saveID, cmd = m.editor.StartSave()
		m.pending.saveInFlight = true
		m.pending.saveRequestID = saveID.RequestID
		return m, cmd
	}
	return m, nil
}

func (m Model) syncCursorToFooter() Model {
	info := m.editor.CursorInfo()
	m.footer, _ = m.footer.Update(footer.UpdateCursorMsg{
		Line:      info.Line,
		Col:       info.Col,
		WordCount: info.WordCount,
	})
	return m
}

// syncDictationAllowed updates the footer's dictation-allowed flag based on
// which pane is focused. Dictation is only available in the editor or chat.
func (m Model) syncDictationAllowed() Model {
	m.footer = m.footer.SetDictationAllowed(m.focus == paneCenter || m.focus == paneChat)
	return m
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

	switch msg := msg.(type) {
	case tea.WindowSizeMsg:
		m.totalWidth, m.totalHeight = msg.Width, msg.Height
		// Size children now (before the generic forward below) so that when the
		// editor receives this WindowSizeMsg its cell footprints are already
		// recomputed and it can re-transmit images at the new size.
		m = m.recalcLayout()

	case tea.KeyPressMsg:
		// Priority 1: Dirty guard — footer consumes all keys.
		if m.footer.InDirtyGuard() {
			m.footer, cmd = m.footer.Update(msg)
			cmds = append(cmds, cmd)
			return m.finalize(cmds)
		}

		// Priority 2: Save in-flight — consume all keys.
		if m.pending != nil && m.pending.saveInFlight {
			return m.finalize(cmds)
		}

		// Priority 3: Global workspace keys.
		consumed := true
		switch {
		case key.Matches(msg, m.keys.TabSwitch):
			if msg.Code >= '1' && msg.Code <= '9' {
				idx := int(msg.Code - '1')
				if path := m.opentabs.PathAt(idx); path != "" {
					m.opentabs = m.opentabs.SelectIndex(idx)
					m, cmd = m.requestOpenPath(path)
					cmds = append(cmds, cmd)
				}
			}

		case key.Matches(msg, m.keys.PinTab):
			m.opentabs = m.opentabs.PinIndex(m.opentabs.Cursor())

		case key.Matches(msg, m.keys.FocusExplorer):
			m.focus = paneTree
			m.leftVisible = true
			m = m.syncDictationAllowed()

		case key.Matches(msg, m.keys.FocusEditor):
			m.focus = paneCenter
			m = m.syncDictationAllowed()

		case key.Matches(msg, m.keys.FocusChat):
			if m.rightVisible && m.focus == paneChat {
				m.rightVisible = false
				m.focus = paneCenter
			} else {
				m.rightVisible = true
				m.focus = paneChat
			}
			m = m.syncDictationAllowed()

		case key.Matches(msg, m.keys.CloseFile):
			m, cmd = m.requestCloseCurrent()
			cmds = append(cmds, cmd)

		case key.Matches(msg, m.keys.ZenMode):
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

		// Priority 4: Editor wants modal input — skip footer help toggle.
		if m.focus == paneCenter && m.editor.WantsModalInput() {
			m.editor = m.editor.SetFocused(true)
			m.editor, cmd = m.editor.Update(msg)
			cmds = append(cmds, cmd)
			m = m.syncCursorToFooter()
			return m.finalize(cmds)
		}

		// Forward to all children; they gate on focused state.
		m.filetree = m.filetree.SetFocused(m.focus == paneTree)
		m.filetree, cmd = m.filetree.Update(msg)
		cmds = append(cmds, cmd)

		m.opentabs = m.opentabs.SetFocused(m.focus == paneTabs)
		m.opentabs, cmd = m.opentabs.Update(msg)
		cmds = append(cmds, cmd)

		// Do not forward keystrokes to the editor during dictation — typing
		// would shift buffer offsets and corrupt the dictation range.
		if m.focus == paneCenter && m.editor.IsDictating() {
			// swallow; dictation text arrives via PartialTranscriptionMsg
		} else {
			m.editor = m.editor.SetFocused(m.focus == paneCenter)
			m.editor, cmd = m.editor.Update(msg)
			cmds = append(cmds, cmd)
		}

		m.chat = m.chat.SetFocused(m.focus == paneChat)
		m.chat, cmd = m.chat.Update(msg)
		cmds = append(cmds, cmd)

		m.footer, cmd = m.footer.Update(msg)
		cmds = append(cmds, cmd)

		m = m.syncCursorToFooter()
		return m.finalize(cmds)

	case filetree.FileSelectedMsg:
		m, cmd = m.requestOpenPath(msg.Path)
		cmds = append(cmds, cmd)

	case filetree.DirSelectedMsg:
		cmds = append(cmds, loadDirCmd(msg.Path, "."))

	case filetree.DirLoadedMsg:
		m.editor = m.editor.SetDir(msg.Root)

	case opentabs.TabSelectedMsg:
		m, cmd = m.requestOpenPath(msg.Path)
		cmds = append(cmds, cmd)

	case editor.LinkClickedMsg:
		if msg.Path != "" {
			m, cmd = m.requestOpenPath(msg.Path)
			cmds = append(cmds, cmd)
		}

	case editor.FileLoadedMsg:
		m.opentabs = m.opentabs.OpenFile(msg.Path)
		m.chat = m.chat.SetFileContext(msg.Path, string(msg.Content))

	case editor.FileRenamedMsg:
		m.opentabs = m.opentabs.RenameFile(msg.OldPath, msg.NewPath)

	case editor.ContentChangedMsg:
		if msg.Dirty {
			m.opentabs = m.opentabs.MarkDirty(msg.Path)
		} else {
			m.opentabs = m.opentabs.MarkClean(msg.Path)
		}
		m.chat = m.chat.SetFileContext(msg.Path, m.editor.Content())

	case editor.FileSavedMsg:
		m.opentabs = m.opentabs.MarkClean(msg.Path)
		if m.pending != nil && m.pending.saveInFlight && m.pending.saveRequestID == msg.RequestID {
			m.pending.saveInFlight = false
			switch m.pending.kind {
			case pendingSwitchFile:
				path := m.pending.path
				m.pending = nil
				cmds = append(cmds, editor.LoadFileCmd(path))
			case pendingCloseFile:
				closePath := m.pending.path
				nextPath := m.pending.nextPath
				m.pending = nil
				m, cmd = m.executeClose(closePath, nextPath)
				cmds = append(cmds, cmd)
			}
		}

	case editor.FileLoadErrorMsg:
		// Temporary: ignore file load errors (e.g. following a broken click link)
		// m.err = msg.Err

	case editor.FileSaveErrorMsg:
		m.err = msg.Err
		if m.pending != nil && m.pending.saveInFlight && m.pending.saveRequestID == msg.RequestID {
			m.pending = nil
		}

	case footer.DirtyGuardResponseMsg:
		m, cmd = m.handleDirtyGuardResponse(msg.Response)
		cmds = append(cmds, cmd)

	case ErrMsg:
		m.err = msg.Err

	case tea.MouseClickMsg:
		// Clear stale drag state on every click. Terminals do not emit a
		// mouse-release event, so a drag that ended on a stationary cursor
		// would otherwise linger until the next motion message.
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
			return m.finalize(cmds)
		}
		if newFocus, ok := m.paneAtPoint(msg.X, msg.Y); ok {
			m.focus = newFocus
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
		return m.finalize(cmds)

	case footer.ConfirmQuitMsg:
		if m.dictCancel != nil {
			m.dictCancel()
			m.dictCancel = nil
		}
		// Clear any inline images from the terminal before exiting. Use
		// tea.Sequence so the delete flushes before tea.Quit (a tea.Batch would
		// race the quit).
		return m, tea.Sequence(m.editor.DeleteAllImagesCmd(), tea.Quit)

	case footer.DictationStartMsg:
		ctx, cancel := context.WithCancel(context.Background())
		m.dictCancel = cancel
		if m.focus == paneCenter {
			m.editor = m.editor.StartDictation()
		}
		cfg := dictation.Config{
			Whisper:  whisper.Client{BaseURL: "http://127.0.0.1:2022", InferencePath: "/v1/audio/transcriptions"},
			Language: inputlang.Current(),
		}
		cmds = append(cmds, dictation.StartCmd(ctx, cfg))

	case footer.DictationStopMsg:
		if m.dictCancel != nil {
			m.dictCancel()
			m.dictCancel = nil
		}
		// dictCh stays alive; FinalTranscriptionMsg arrives via pending ListenCmd.

	case dictation.ReadyMsg:
		m.dictCh = msg.Ch
		cmds = append(cmds, dictation.ListenCmd(m.dictCh))

	case dictation.PartialTranscriptionMsg:
		if m.focus == paneCenter {
			m.editor = m.editor.ApplyDictationChunk(msg.Accumulated)
			path := m.editor.FilePath()
			cmds = append(cmds, func() tea.Msg {
				return editor.ContentChangedMsg{Path: path, Dirty: true}
			})
		} else if m.focus == paneChat {
			m.chat = m.chat.SetDictationPartial(msg.Accumulated)
		}
		cmds = append(cmds, dictation.ListenCmd(m.dictCh))

	case dictation.FinalTranscriptionMsg:
		m.footer = m.footer.SetDictating(false)
		m.dictCh = nil
		if m.focus == paneCenter {
			m.editor = m.editor.FinalizeDictation()
		} else if m.focus == paneChat {
			m.chat = m.chat.FinalizeDictation(msg.Text)
		}

	case dictation.ErrorMsg:
		if msg.Fatal {
			if m.dictCancel != nil {
				m.dictCancel()
				m.dictCancel = nil
			}
			m.dictCh = nil
			m.footer = m.footer.SetDictating(false)
			m.editor = m.editor.CancelDictation()
			m.chat = m.chat.CancelDictation()
		} else {
			cmds = append(cmds, dictation.ListenCmd(m.dictCh))
		}
	}

	// Forward non-key messages to children.
	if _, isKey := msg.(tea.KeyPressMsg); !isKey {
		m.filetree = m.filetree.SetFocused(m.focus == paneTree)
		m.filetree, cmd = m.filetree.Update(msg)
		cmds = append(cmds, cmd)

		m.opentabs = m.opentabs.SetFocused(m.focus == paneTabs)
		m.opentabs, cmd = m.opentabs.Update(msg)
		cmds = append(cmds, cmd)

		m.editor = m.editor.SetFocused(m.focus == paneCenter)
		m.editor, cmd = m.editor.Update(msg)
		cmds = append(cmds, cmd)

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

	centerBlock := borderStyle(m.focus == paneCenter, m.styles).
		Width(centerW).Height(contentH).
		Render(m.editor.View())

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
		return tea.NewView(lipgloss.JoinVertical(lipgloss.Left, errLine, body, m.footer.View()))
	}
	return tea.NewView(lipgloss.JoinVertical(lipgloss.Left, body, m.footer.View()))
}

func borderStyle(active bool, st styles.Styles) lipgloss.Style {
	if active {
		return st.ActiveBorder
	}
	return st.InactiveBorder
}

func loadDirCmd(dir string, initialRoot string) tea.Cmd {
	return func() tea.Msg {
		des, err := os.ReadDir(dir)
		if err != nil {
			return ErrMsg{Err: fmt.Errorf("load dir %q: %w", dir, err)}
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
		return filetree.DirLoadedMsg{Root: dir, Entries: entries}
	}
}
