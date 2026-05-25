package workspace

import (
	"fmt"
	"os"
	"path/filepath"

	"charm.land/bubbles/v2/key"
	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"

	"rune/pkg/command"
	"rune/pkg/editor/keybind"
	"rune/pkg/ui/components/editor"
	"rune/pkg/ui/components/filetree"
	"rune/pkg/ui/components/footer"
	"rune/pkg/ui/components/opentabs"
	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"
)

type pane int

const (
	paneTree pane = iota
	paneTabs
	paneCenter
)

func (p pane) isLeft() bool { return p == paneTree || p == paneTabs }

// ErrMsg signals an I/O error to the workspace page.
type ErrMsg struct{ Err error }

const defaultLeftPaneW = 22

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
	err                     error
	keys                    keymap.Bindings
	styles                  styles.Styles
	pending                 *pendingDirtyAction
}

func New(keys keymap.Bindings, st styles.Styles, reg command.Registry, resolver keybind.Resolver) Model {
	return Model{
		filetree:    filetree.New(keys, st),
		opentabs:    opentabs.New(keys, st),
		editor:      editor.New(keys, st, reg, resolver),
		footer:      footer.New(keys, st).SetHelp(keys.HelpText()),
		focus:       paneTree,
		leftVisible: true,
		leftPaneW:   defaultLeftPaneW,
		keys:        keys,
		styles:      st,
	}
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
	centerW := m.totalWidth - leftW
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

	otH := m.opentabs.Height()
	ftH := innerH - otH
	if ftH < 4 {
		ftH = 4
	}
	m.filetree = m.filetree.SetSize(innerLeftW, ftH)
	m.opentabs = m.opentabs.SetSize(innerLeftW, otH)
	m.editor = m.editor.SetSize(innerCenterW, innerH)
	m.footer = m.footer.SetSize(m.totalWidth, m.footer.Height())
	return m
}

func (m Model) Init() tea.Cmd {
	return tea.Batch(
		m.filetree.Init(),
		m.opentabs.Init(),
		m.editor.Init(),
		m.footer.Init(),
		loadDirCmd(".", "."),
	)
}

func (m Model) requestOpenPath(path string) (Model, tea.Cmd) {
	if path == m.editor.OpenPath() {
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
	currentPath := m.editor.OpenPath()
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
		m.opentabs = m.opentabs.MarkClean(m.editor.OpenPath())
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

		case key.Matches(msg, m.keys.FocusEditor):
			m.focus = paneCenter

		case key.Matches(msg, m.keys.CloseFile):
			m, cmd = m.requestCloseCurrent()
			cmds = append(cmds, cmd)

		case key.Matches(msg, m.keys.ZenMode):
			m.leftVisible = !m.leftVisible
			if !m.leftVisible && m.focus.isLeft() {
				m.focus = paneCenter
			}

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

		m.editor = m.editor.SetFocused(m.focus == paneCenter)
		m.editor, cmd = m.editor.Update(msg)
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

	case editor.FileLoadedMsg:
		m.opentabs = m.opentabs.OpenFile(msg.Path)

	case editor.ContentChangedMsg:
		if msg.Dirty {
			m.opentabs = m.opentabs.MarkDirty(msg.Path)
		} else {
			m.opentabs = m.opentabs.MarkClean(msg.Path)
		}

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
		m.err = msg.Err

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

	case footer.ConfirmQuitMsg:
		return m, tea.Quit
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
	centerW := m.totalWidth - leftW
	if centerW < 0 {
		centerW = 0
	}

	centerBlock := borderStyle(m.focus == paneCenter, m.styles).
		Width(centerW).Height(contentH).
		Render(m.editor.View())

	var body string
	if m.leftVisible {
		leftContent := lipgloss.JoinVertical(lipgloss.Left,
			m.filetree.View(),
			m.opentabs.View(),
		)
		leftBlock := borderStyle(m.focus.isLeft(), m.styles).
			Width(leftW).Height(contentH).
			Render(leftContent)
		body = lipgloss.JoinHorizontal(lipgloss.Top, leftBlock, centerBlock)
	} else {
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
		return filetree.DirLoadedMsg{Root: dir, Entries: entries}
	}
}
