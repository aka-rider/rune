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

	"rune/pkg/command"
	"rune/pkg/dictation"
	"rune/pkg/editor/keybind"
	"rune/pkg/inputlang"
	"rune/pkg/merge"
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

var dirtyGuardOptions = []footer.GuardOption{
	{Key: 's', Response: footer.DataLossSave},
	{Key: 'd', Response: footer.DataLossDiscard},
}

var mergeGuardOptions = []footer.GuardOption{
	{Key: 'y', Response: footer.DataLossMergeAccept},
	{Key: 'n', Response: footer.DataLossMergeReject},
}

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
	watchedDir              string             // current directory being watched
	cancelWatch             context.CancelFunc // cancels active watchDirCmd (§6.3)
	origContent             []byte             // ancestor for 3-way merge (last known disk content)
	pendingMergeContent     []byte             // "theirs" — external changes awaiting user decision
	watchedFilePath         string             // current file being watched (for lifecycle tracking)
	cancelFileWatch         context.CancelFunc // cancels active file watcher
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
	topOffset := 1
	if m.err != nil {
		topOffset = 2 // error line above body shifts editor content down one row
	}
	m.editor = m.editor.SetOffset(leftW+1, topOffset)
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
		m.footer = m.footer.SetGuard(footer.GuardDirty, dirtyGuardOptions)
		return m, nil
	}
	var cmds []tea.Cmd
	var watchCmd tea.Cmd
	if path != "" {
		m, watchCmd = m.startFileWatch(path)
		cmds = append(cmds, watchCmd)
	}
	cmds = append(cmds, editor.LoadFileCmd(path))
	return m, tea.Batch(cmds...)
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
		m.footer = m.footer.SetGuard(footer.GuardDirty, dirtyGuardOptions)
		m = m.stopFileWatch()
		return m, nil
	}
	m = m.stopFileWatch()
	return m.executeClose(currentPath, nextPath)
}

func (m Model) executeClose(closePath, nextPath string) (Model, tea.Cmd) {
	var cmds []tea.Cmd
	var cmd, watchCmd tea.Cmd
	m.opentabs = m.opentabs.CloseFile(closePath)
	m.editor, cmd = m.editor.Update(editor.FileClosedMsg{Path: closePath})
	cmds = append(cmds, cmd)
	if nextPath != "" {
		m, watchCmd = m.startFileWatch(nextPath)
		cmds = append(cmds, watchCmd)
		cmds = append(cmds, editor.LoadFileCmd(nextPath))
	} else {
		// Last tab closed — reset to a fresh untitled buffer.
		var createCmd tea.Cmd
		m, createCmd = m.CreateUntitled()
		cmds = append(cmds, createCmd)
	}
	return m, tea.Batch(cmds...)
}

func (m Model) handleDataLossGuardResponse(resp footer.DataLossGuardResponse) (Model, tea.Cmd) {
	var cmd, watchCmd tea.Cmd
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
		m.editor = m.editor.ApplyMergeResult(ours, result.Output)
		m.origContent = result.Output // merged result becomes new ancestor
		return m, func() tea.Msg { return editor.FileMergedMsg{
			Path:       m.editor.FilePath(),
			Content:    result.Output,
			Conflicted: result.Conflicted,
		}}

	case footer.DataLossMergeReject:
		diskContent := m.pendingMergeContent
		m.pendingMergeContent = nil
		m.footer = m.footer.SetGuard(0, nil)
		m.origContent = diskContent // disk state becomes new ancestor for next merge
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
		m.opentabs = m.opentabs.MarkClean(m.editor.FilePath())
		switch m.pending.kind {
		case pendingSwitchFile:
			path := m.pending.path
			m.pending = nil
			m, watchCmd = m.startFileWatch(path)
			return m, tea.Batch(watchCmd, editor.LoadFileCmd(path))
		case pendingCloseFile:
			closePath := m.pending.path
			nextPath := m.pending.nextPath
			m.pending = nil
			return m.executeClose(closePath, nextPath)
		}

	case footer.DataLossSave:
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

	switch msg := msg.(type) {
	case tea.WindowSizeMsg:
		m.totalWidth, m.totalHeight = msg.Width, msg.Height
		// Size children now (before the generic forward below) so that when the
		// editor receives this WindowSizeMsg its cell footprints are already
		// recomputed and it can re-transmit images at the new size.
		m = m.recalcLayout()

	case tea.KeyPressMsg:
		// Priority 1: Dirty guard — footer consumes all keys.
		if m.footer.InGuard() {
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
		m, cmd = m.startWatch(msg.Root)
		cmds = append(cmds, cmd)

	case dirChangedMsg:
		dir := m.watchedDir
		cmds = append(cmds, reloadDirCmd(dir, "."))

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
		m.origContent = msg.Content

	case editor.FileChangedOnDiskMsg:
		if m.editor.FilePath() == "" || m.editor.FilePath() != msg.Path {
			return m, nil // not current file or untitled, ignore
		}
		// Pre-check: if disk content matches the buffer, no merge needed.
		// This handles the race where a save completes and the watcher reads
		// the same content, or external tools touch the file without changes.
		if string(msg.NewContent) == m.editor.Content() {
			m.origContent = msg.NewContent
			return m, nil
		}
		m.pendingMergeContent = msg.NewContent
		m.footer = m.footer.SetGuard(footer.GuardMerge, mergeGuardOptions)

	case editor.FileMergedMsg:
		m.opentabs = m.opentabs.MarkDirty(msg.Path)

	case editor.FileRenamedMsg:
		m.opentabs = m.opentabs.RenameFile(msg.OldPath, msg.NewPath)

	case editor.FileRenameErrorMsg:
		m.footer, cmd = m.footer.Update(footer.ShowErrorMsg{Text: msg.Err.Error()})
		cmds = append(cmds, cmd)

	case editor.UntitledRenameMsg:
		// Untitled file gets a name — create on disk
		dir := m.currentDir()
		newPath := filepath.Join(dir, msg.Name+".md")
		m.editor = m.editor.SetFilePath(newPath)
		m.opentabs = m.opentabs.OpenFile(newPath)
		cmds = append(cmds, createFileCmd(newPath, m.editor.Content()))

	case fileCreatedMsg:
		if msg.err != nil {
			m.footer, cmd = m.footer.Update(footer.ShowErrorMsg{Text: msg.err.Error()})
			cmds = append(cmds, cmd)
		} else {
			// File committed to disk — use buffer content as the merge ancestor.
			m.origContent = []byte(m.editor.Content())
		}

	case editor.ContentChangedMsg:
		if msg.Dirty {
			m.opentabs = m.opentabs.MarkDirty(msg.Path)
			// First content edit on untitled file → create file on disk
			if msg.Path == "" && m.editor.Content() != "" {
				dir := m.currentDir()
				name := m.editor.TitleText()
				newPath := filepath.Join(dir, name+".md")
				m.editor = m.editor.SetFilePath(newPath)
				m.opentabs = m.opentabs.OpenFile(newPath)
				cmds = append(cmds, createFileCmd(newPath, m.editor.Content()))
			}
		} else {
			m.opentabs = m.opentabs.MarkClean(msg.Path)
		}
		m.chat = m.chat.SetFileContext(msg.Path, m.editor.Content())

	case editor.FileSavedMsg:
		m.opentabs = m.opentabs.MarkClean(msg.Path)
		if m.pending != nil && m.pending.saveInFlight && m.pending.saveRequestID == msg.RequestID {
			m.pending.saveInFlight = false
			var watchCmd tea.Cmd
			switch m.pending.kind {
			case pendingSwitchFile:
				path := m.pending.path
				m.pending = nil
				m, watchCmd = m.startFileWatch(path)
				cmds = append(cmds, watchCmd, editor.LoadFileCmd(path))
			case pendingCloseFile:
				closePath := m.pending.path
				nextPath := m.pending.nextPath
				m.pending = nil
				m, cmd = m.executeClose(closePath, nextPath)
				cmds = append(cmds, cmd)
			}
		}
		// After save, buffer equals disk — update origContent as the new ancestor.
		m.origContent = []byte(m.editor.Content())

	case editor.FileLoadErrorMsg:
		// Temporary: ignore file load errors (e.g. following a broken click link)
		// m.err = msg.Err

	case editor.FileSaveErrorMsg:
		m.err = msg.Err
		if m.pending != nil && m.pending.saveInFlight && m.pending.saveRequestID == msg.RequestID {
			m.pending = nil
		}

	case fileWatchReadError:
		m.err = fmt.Errorf("external change to %s: %w", msg.path, msg.err)

	case footer.DataLossGuardResponseMsg:
		m, cmd = m.handleDataLossGuardResponse(msg.Response)
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
			return m.finalizeLayoutChange(cmds)
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
		return m.finalizeLayoutChange(cmds)

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
	centerBlock = overlayBreadcrumb(centerBlock, m.editor.BreadcrumbView(), m.focus == paneCenter, m.styles)

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

	// Inline image escape sequences are NOT appended to the frame: the editor
	// emits them via tea.Raw from its Update (written straight to the tty,
	// bypassing the cell renderer). The workspace already batches the editor's
	// returned Cmds, so no extra wiring is needed here.
	if m.err != nil {
		errLine := m.styles.Error.Render("error: " + m.err.Error())
		frame := lipgloss.JoinVertical(lipgloss.Left, errLine, body, m.footer.View())
		return tea.NewView(frame)
	}
	frame := lipgloss.JoinVertical(lipgloss.Left, body, m.footer.View())
	return tea.NewView(frame)
}

// overlayBreadcrumb post-processes a rendered bordered block by replacing part of
// the bottom border line with the breadcrumb text, right-aligned before "──╯".
// This avoids disabling BorderBottom or manual corner construction which caused
// alignment bugs. If the breadcrumb is empty or too wide, the border is unchanged.
func overlayBreadcrumb(block, breadcrumb string, active bool, st styles.Styles) string {
	if breadcrumb == "" {
		return block
	}

	lines := strings.Split(block, "\n")
	if len(lines) < 2 {
		return block
	}

	lastIdx := len(lines) - 1
	bottomLine := lines[lastIdx]
	borderW := lipgloss.Width(bottomLine)

	bcWidth := lipgloss.Width(breadcrumb)
	// Need space for: ╰ + at least 1 dash + space + breadcrumb + space + ── + ╯
	// Minimum overhead: corner(1) + dash(1) + space(1) + space(1) + dashes(2) + corner(1) = 7
	minOverhead := 7
	if bcWidth+minOverhead > borderW {
		return block // breadcrumb too wide, skip overlay
	}

	// Determine border color
	borderColor := st.InactiveBorder.GetBorderTopForeground()
	if active {
		borderColor = st.ActiveBorder.GetBorderTopForeground()
	}
	bStyle := lipgloss.NewStyle().Foreground(borderColor)

	// Build the custom bottom line: ╰───── breadcrumb ──╯
	// Right-aligned: breadcrumb sits before "──╯"
	rightPad := bStyle.Render("──╯")
	leftCorner := bStyle.Render("╰")
	content := " " + breadcrumb + " "
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

// readDirEntries reads and sorts the directory listing for dir.
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

// watchDirCmd watches a directory for Create/Remove/Rename events and returns
// dirChangedMsg when the listing should be refreshed. One-shot: returns after
// the first event batch (with 50ms debounce). Caller restarts via DirLoadedMsg.
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
					// Debounce: coalesce burst events for 50ms.
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
				// Ignore Write/Chmod — they don't affect directory listing.
			case _, ok := <-watcher.Errors:
				if !ok {
					return nil
				}
				return nil // fire-and-forget: watcher error
			}
		}
	}
}

// startWatch cancels any active directory watcher and starts a new one for dir.
func (m Model) startWatch(dir string) (Model, tea.Cmd) {
	if m.cancelWatch != nil {
		m.cancelWatch()
	}
	ctx, cancel := context.WithCancel(context.Background())
	m.cancelWatch = cancel
	m.watchedDir = dir
	return m, watchDirCmd(ctx, dir)
}

// watchFileCmd watches a single file for Write events and returns FileChangedOnDiskMsg
// when the file content changes. Ignores Create/Remove/Rename (those are handled by
// the directory watcher). Context-cancellable.
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
					// Read the new content and emit.
					b, err := os.ReadFile(path)
					if err != nil {
						return fileWatchReadError{path: path, err: fmt.Errorf("read %q: %w", path, err)}
					}
					return editor.FileChangedOnDiskMsg{Path: path, NewContent: b}
				}
				// Ignore Create/Remove/Rename — directory watcher handles those.
			case _, ok := <-watcher.Errors:
				if !ok {
					return nil
				}
				return nil // fire-and-forget: watcher error
			}
		}
	}
}

// startFileWatch cancels any active file watcher and starts a new one for path.
func (m Model) startFileWatch(path string) (Model, tea.Cmd) {
	if m.cancelFileWatch != nil {
		m.cancelFileWatch()
	}
	ctx, cancel := context.WithCancel(context.Background())
	m.cancelFileWatch = cancel
	m.watchedFilePath = path
	return m, watchFileCmd(ctx, path)
}

// stopFileWatch cancels and clears the active file watcher.
func (m Model) stopFileWatch() Model {
	if m.cancelFileWatch != nil {
		m.cancelFileWatch()
		m.cancelFileWatch = nil
		m.watchedFilePath = ""
	}
	return m
}

// fileCreatedMsg reports the result of creating a new file on disk.
type fileCreatedMsg struct {
	path string
	err  error
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
		return fileCreatedMsg{path: path}
	}
}

// nextUntitled returns a placeholder name for a new untitled file in dir.
// It finds the lowest N >= 1 such that "Untitled N.md" does not exist in dir,
// or exists but is empty (zero bytes). This matches the expected UX: Untitled 1
// by default; if Untitled 1 is non-empty, the next new file is Untitled 2, etc.
func nextUntitled(dir string) string {
	for n := 1; ; n++ {
		name := fmt.Sprintf("Untitled %d", n)
		info, err := os.Stat(filepath.Join(dir, name+".md"))
		if err != nil || info.Size() == 0 {
			// File doesn't exist or is empty — this slot is available.
			return name
		}
	}
}

// CreateUntitled opens a new untitled buffer in the current filetree directory.
func (m Model) CreateUntitled() (Model, tea.Cmd) {
	dir := m.currentDir()
	name := nextUntitled(dir)
	m.editor = m.editor.SetContent("", nil)
	m.editor = m.editor.SetTitle(name)
	m.opentabs = m.opentabs.OpenFile("")
	m.focus = paneCenter
	return m, nil
}
