package editor

import (
	"fmt"
	"os"

	"charm.land/bubbles/v2/viewport"
	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"

	"rune/pkg/ui/components/breadcrumb"
	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"
)

// FileLoadedMsg is produced by LoadFileCmd and consumed by this component.
type FileLoadedMsg struct{ Path, Content string }

// FileClosedMsg signals that a file's editor content should be cleared.
type FileClosedMsg struct{ Path string }

type Model struct {
	viewport   viewport.Model
	breadcrumb breadcrumb.Model
	openPath   string
	width      int
	height     int
	focused    bool
	keys       keymap.Bindings
	styles     styles.Styles
}

func New(keys keymap.Bindings, st styles.Styles) Model {
	vp := viewport.New()
	vp.MouseWheelEnabled = true
	return Model{
		viewport:   vp,
		breadcrumb: breadcrumb.New(st),
		keys:       keys,
		styles:     st,
	}
}

func (m Model) SetSize(w, h int) Model {
	m.width = w
	m.height = h
	m.breadcrumb = m.breadcrumb.SetSize(w, 1)
	vpH := h - m.breadcrumb.Height()
	if vpH < 1 {
		vpH = 1
	}
	m.viewport.SetWidth(w)
	m.viewport.SetHeight(vpH)
	return m
}

func (m Model) SetFocused(f bool) Model { m.focused = f; return m }
func (m Model) Height() int             { return m.height }
func (m Model) OpenPath() string        { return m.openPath }

func (m Model) SetDir(dir string) Model {
	m.breadcrumb = m.breadcrumb.SetDir(dir)
	return m
}

func (m Model) Init() tea.Cmd { return nil }

func (m Model) Update(msg tea.Msg) (Model, tea.Cmd) {
	var cmd tea.Cmd

	switch msg := msg.(type) {
	case FileLoadedMsg:
		m.openPath = msg.Path
		m.viewport.SetContent(msg.Content)
		m.viewport.GotoTop()
		m.breadcrumb = m.breadcrumb.SetPath(msg.Path)

	case FileClosedMsg:
		if msg.Path == m.openPath {
			m.openPath = ""
			m.viewport.SetContent("")
			m.breadcrumb = m.breadcrumb.SetPath("")
		}

	case tea.KeyPressMsg:
		if !m.focused {
			return m, nil
		}
		m.viewport, cmd = m.viewport.Update(msg)
		return m, cmd

	default:
		m.viewport, cmd = m.viewport.Update(msg)
		return m, cmd
	}

	return m, nil
}

func (m Model) View() string {
	content := lipgloss.JoinVertical(lipgloss.Left,
		m.breadcrumb.View(),
		m.viewport.View(),
	)
	return lipgloss.NewStyle().
		MaxWidth(m.width).
		MaxHeight(m.height).
		Height(m.height).
		Render(content)
}

// LoadFileCmd returns a Cmd that reads a file and produces a FileLoadedMsg.
func LoadFileCmd(path string) tea.Cmd {
	return func() tea.Msg {
		b, err := os.ReadFile(path)
		if err != nil {
			return ErrMsg{Err: fmt.Errorf("open %q: %w", path, err)}
		}
		return FileLoadedMsg{Path: path, Content: string(b)}
	}
}

// ErrMsg signals an I/O error during file loading.
type ErrMsg struct{ Err error }

// PreferredWidth returns the minimum comfortable width for the editor pane.
func PreferredWidth() int { return 40 }
