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

// leftPaneWidth is the default width for the sidebar. It is a Model field
// so it can be adjusted at runtime (e.g., user resize). This is not a
// package-level constant — components expose Height()/Width() for intrinsic sizing.
const defaultLeftPaneW = 22

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

	// Subtract border cells (1 left + 1 right, 1 top + 1 bottom = 2 each axis)
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

func (m Model) Update(msg tea.Msg) (Model, tea.Cmd) {
	var cmds []tea.Cmd
	var cmd tea.Cmd

	switch msg := msg.(type) {
	case tea.WindowSizeMsg:
		m.totalWidth, m.totalHeight = msg.Width, msg.Height

	case tea.KeyPressMsg:
		switch {
		case key.Matches(msg, m.keys.TabSwitch):
			if msg.Code >= '1' && msg.Code <= '9' {
				idx := int(msg.Code - '1')
				if path := m.opentabs.PathAt(idx); path != "" {
					m.opentabs = m.opentabs.SelectIndex(idx)
					cmds = append(cmds, editor.LoadFileCmd(path))
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
			if path := m.editor.OpenPath(); path != "" {
				m.opentabs = m.opentabs.CloseFile(path)
				m.editor, cmd = m.editor.Update(editor.FileClosedMsg{Path: path})
				cmds = append(cmds, cmd)
			}


		case key.Matches(msg, m.keys.ZenMode):
			m.leftVisible = !m.leftVisible
			if !m.leftVisible && m.focus.isLeft() {
				m.focus = paneCenter
			}
		}

	case filetree.FileSelectedMsg:
		cmds = append(cmds, editor.LoadFileCmd(msg.Path))

	case filetree.DirSelectedMsg:
		cmds = append(cmds, loadDirCmd(msg.Path, "."))

	case filetree.DirLoadedMsg:
		m.editor = m.editor.SetDir(msg.Root)

	case opentabs.TabSelectedMsg:
		cmds = append(cmds, editor.LoadFileCmd(msg.Path))

	case editor.FileLoadedMsg:
		m.opentabs = m.opentabs.OpenFile(msg.Path)

	case editor.FileLoadErrorMsg:
		m.err = msg.Err
	case editor.FileSaveErrorMsg:
		m.err = msg.Err

	case ErrMsg:
		m.err = msg.Err

	case footer.ConfirmQuitMsg:
		return m, tea.Quit
	}

	// Update children — set focus before forwarding messages
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

	if m.totalWidth > 0 {
		m = m.recalcLayout()
	}

	return m, tea.Batch(cmds...)
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
