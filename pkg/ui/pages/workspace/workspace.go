package workspace

import (
	"fmt"
	"os"
	"path/filepath"

	"charm.land/bubbles/v2/key"
	"charm.land/bubbles/v2/viewport"
	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"

	"rune/pkg/ui/components/breadcrumb"
	"rune/pkg/ui/components/filetree"
	"rune/pkg/ui/components/footer"
	"rune/pkg/ui/components/opentabs"
	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"
)

type pane int

const (
	paneTree   pane = iota
	paneTabs
	paneCenter
)

func (p pane) isLeft() bool { return p == paneTree || p == paneTabs }

type FileLoadedMsg struct{ Path, Content string }
type ErrMsg struct{ Err error }

const leftPaneW = 22
const footerH = 1

type Model struct {
	totalWidth, totalHeight int
	filetree                filetree.Model
	opentabs                opentabs.Model
	breadcrumb              breadcrumb.Model
	viewport                viewport.Model
	footer                  footer.Model
	focus                   pane
	leftVisible             bool
	openPath                string
	err                     error
	keys                    keymap.Bindings
	styles                  styles.Styles
}

func New(keys keymap.Bindings, st styles.Styles) Model {
	vp := viewport.New()
	vp.MouseWheelEnabled = true
	return Model{
		filetree:    filetree.New(keys, st),
		opentabs:    opentabs.New(keys, st),
		breadcrumb:  breadcrumb.New(st),
		viewport:    vp,
		footer:      footer.New(keys, st),
		focus:       paneTree,
		leftVisible: true,
		keys:        keys,
		styles:      st,
	}
}

func (m Model) dims() (leftW, centerW, contentH int) {
	contentH = m.totalHeight - footerH
	if contentH < 0 {
		contentH = 0
	}
	if m.leftVisible {
		leftW = leftPaneW
	}
	centerW = m.totalWidth - leftW
	if centerW < 0 {
		centerW = 0
	}
	return
}

func (m Model) recalcLayout() Model {
	leftW, centerW, contentH := m.dims()

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

	m.breadcrumb = m.breadcrumb.SetSize(innerCenterW, 1)
	vpH := innerH - m.breadcrumb.Height()
	if vpH < 1 {
		vpH = 1
	}
	m.viewport.SetWidth(innerCenterW)
	m.viewport.SetHeight(vpH)

	m.footer = m.footer.SetSize(m.totalWidth, 1)
	return m
}

func (m Model) Init() tea.Cmd {
	return tea.Batch(
		m.filetree.Init(),
		m.opentabs.Init(),
		m.breadcrumb.Init(),
		m.footer.Init(),
		loadDirCmd("."),
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
		case key.Matches(msg, m.keys.Quit):
			return m, tea.Quit

		case key.Matches(msg, m.keys.CloseFile):
			if m.openPath != "" {
				path := m.openPath
				cmds = append(cmds, func() tea.Msg { return opentabs.FileClosedMsg{Path: path} })
			}

		case key.Matches(msg, m.keys.FocusLeft):
			if m.focus.isLeft() && m.leftVisible {
				m.leftVisible = false
				m.focus = paneCenter
			} else {
				m.leftVisible = true
				m.focus = paneTree
			}

		case key.Matches(msg, m.keys.FocusCenter):
			m.focus = paneCenter

		case key.Matches(msg, m.keys.CycleLeftFocus):
			if m.focus == paneTree {
				m.focus = paneTabs
			} else if m.focus == paneTabs {
				m.focus = paneTree
			}

		case key.Matches(msg, m.keys.ZenMode):
			m.leftVisible = !m.leftVisible
			if !m.leftVisible && m.focus.isLeft() {
				m.focus = paneCenter
			}
		}

	case filetree.FileSelectedMsg:
		cmds = append(cmds, loadFileCmd(msg.Path))

	case opentabs.TabSelectedMsg:
		cmds = append(cmds, loadFileCmd(msg.Path))

	case FileLoadedMsg:
		m.openPath = msg.Path
		m.viewport.SetContent(msg.Content)
		m.viewport.GotoTop()
		m.breadcrumb = m.breadcrumb.SetPath(msg.Path)
		m.opentabs, cmd = m.opentabs.Update(opentabs.FileOpenedMsg{Path: msg.Path})
		cmds = append(cmds, cmd)

	case opentabs.FileClosedMsg:
		if msg.Path == m.openPath {
			m.openPath = ""
			m.viewport.SetContent("")
			m.breadcrumb = m.breadcrumb.SetPath("")
		}

	case ErrMsg:
		m.err = msg.Err
	}

	m.filetree = m.filetree.SetFocused(m.focus == paneTree)
	m.filetree, cmd = m.filetree.Update(msg)
	cmds = append(cmds, cmd)

	m.opentabs = m.opentabs.SetFocused(m.focus == paneTabs)
	m.opentabs, cmd = m.opentabs.Update(msg)
	cmds = append(cmds, cmd)

	m.breadcrumb, cmd = m.breadcrumb.Update(msg)
	cmds = append(cmds, cmd)

	m.footer, cmd = m.footer.Update(msg)
	cmds = append(cmds, cmd)

	switch msg.(type) {
	case tea.KeyPressMsg:
		if m.focus == paneCenter {
			m.viewport, cmd = m.viewport.Update(msg)
			cmds = append(cmds, cmd)
		}
	default:
		m.viewport, cmd = m.viewport.Update(msg)
		cmds = append(cmds, cmd)
	}

	if m.totalWidth > 0 {
		m = m.recalcLayout()
	}

	return m, tea.Batch(cmds...)
}

func (m Model) View() tea.View {
	if m.totalWidth == 0 {
		return tea.NewView("")
	}
	leftW, centerW, contentH := m.dims()

	centerContent := lipgloss.JoinVertical(lipgloss.Left,
		m.breadcrumb.View(),
		m.viewport.View(),
	)
	centerBlock := borderStyle(m.focus == paneCenter, m.styles).
		Width(centerW - 2).Height(contentH - 2).
		Render(centerContent)

	var body string
	if m.leftVisible {
		leftContent := lipgloss.JoinVertical(lipgloss.Left,
			m.filetree.View(),
			m.opentabs.View(),
		)
		leftBlock := borderStyle(m.focus.isLeft(), m.styles).
			Width(leftW - 2).Height(contentH - 2).
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

func loadDirCmd(dir string) tea.Cmd {
	return func() tea.Msg {
		des, err := os.ReadDir(dir)
		if err != nil {
			return ErrMsg{Err: fmt.Errorf("load dir %q: %w", dir, err)}
		}
		entries := make([]filetree.Entry, 0, len(des))
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

func loadFileCmd(path string) tea.Cmd {
	return func() tea.Msg {
		b, err := os.ReadFile(path)
		if err != nil {
			return ErrMsg{Err: fmt.Errorf("open %q: %w", path, err)}
		}
		return FileLoadedMsg{Path: path, Content: string(b)}
	}
}
