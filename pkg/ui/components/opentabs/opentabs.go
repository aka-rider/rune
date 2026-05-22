package opentabs

import (
	"strings"

	"charm.land/bubbles/v2/key"
	tea "charm.land/bubbletea/v2"

	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"
)

type FileOpenedMsg struct{ Path string }
type FileClosedMsg struct{ Path string }
type TabSelectedMsg struct{ Path string }

type Tab struct {
	Path   string
	Name   string
	Pinned bool
	Dirty  bool
	Active bool
}

type Model struct {
	tabs    []Tab
	cursor  int
	width   int
	height  int
	focused bool
	keys    keymap.Bindings
	styles  styles.Styles
}

func New(keys keymap.Bindings, st styles.Styles) Model {
	return Model{keys: keys, styles: st}
}

func (m Model) SetSize(w, h int) Model  { m.width = w; m.height = h; return m }
func (m Model) SetFocused(f bool) Model { m.focused = f; return m }

func (m Model) Height() int {
	if len(m.tabs) == 0 {
		return 1
	}
	return len(m.tabs) + 1
}

func (m Model) Init() tea.Cmd { return nil }

func (m Model) Update(msg tea.Msg) (Model, tea.Cmd) {
	switch msg := msg.(type) {
	case FileOpenedMsg:
		found := false
		for i := range m.tabs {
			m.tabs[i].Active = m.tabs[i].Path == msg.Path
			if m.tabs[i].Path == msg.Path {
				found = true
				m.cursor = i
			}
		}
		if !found {
			name := tabName(msg.Path)
			newTab := Tab{Path: msg.Path, Name: name, Active: true}
			for i := range m.tabs {
				m.tabs[i].Active = false
			}
			m.tabs = append(m.tabs, newTab)
			m.cursor = len(m.tabs) - 1
		}

	case FileClosedMsg:
		for i, t := range m.tabs {
			if t.Path == msg.Path {
				m.tabs = append(m.tabs[:i], m.tabs[i+1:]...)
				if m.cursor >= len(m.tabs) && m.cursor > 0 {
					m.cursor = len(m.tabs) - 1
				}
				break
			}
		}

	case tea.KeyPressMsg:
		if !m.focused || len(m.tabs) == 0 {
			break
		}
		switch {
		case key.Matches(msg, m.keys.Up):
			if m.cursor > 0 {
				m.cursor--
			}
		case key.Matches(msg, m.keys.Down):
			if m.cursor < len(m.tabs)-1 {
				m.cursor++
			}
		case key.Matches(msg, m.keys.Select):
			path := m.tabs[m.cursor].Path
			return m, func() tea.Msg { return TabSelectedMsg{Path: path} }
		}
	}
	return m, nil
}

func (m Model) View() string {
	var b strings.Builder

	b.WriteString(m.styles.TabsDivider.Render("── Open ──────"))

	for i, t := range m.tabs {
		b.WriteByte('\n')

		prefix := "  "
		if i == m.cursor && m.focused {
			prefix = "> "
		}
		b.WriteString(prefix)

		name := t.Name
		if t.Pinned {
			name = m.styles.TabPinned.Render("★") + " " + name
		}
		if t.Dirty {
			name = name + " " + m.styles.TabDirty.Render("●")
		}

		if t.Active {
			b.WriteString(m.styles.TabActive.Render(name))
		} else {
			b.WriteString(m.styles.TabNormal.Render(name))
		}
	}

	return b.String()
}

func tabName(path string) string {
	if path == "" {
		return "(untitled)"
	}
	parts := strings.Split(path, "/")
	name := parts[len(parts)-1]
	if name == "" {
		return path
	}
	return name
}
