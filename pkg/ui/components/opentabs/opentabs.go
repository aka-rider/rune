package opentabs

import (
	"fmt"
	"strings"

	"charm.land/bubbles/v2/key"
	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"

	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"
)

// TabSelectedMsg is emitted when the user explicitly selects a tab via
// keyboard navigation within the focused opentabs component.
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
func (m Model) Focused() bool           { return m.focused }
func (m Model) Cursor() int             { return m.cursor }

func (m Model) Height() int {
	if len(m.tabs) == 0 {
		return 1
	}
	return len(m.tabs) + 1
}

// PathAt returns the file path at the given tab index, or "" if out of bounds.
func (m Model) PathAt(index int) string {
	if index < 0 || index >= len(m.tabs) {
		return ""
	}
	return m.tabs[index].Path
}

// SelectIndex switches the active tab to the given index. Returns the updated
// model. The page calls this directly — no message round-trip needed.
func (m Model) SelectIndex(index int) Model {
	if index < 0 || index >= len(m.tabs) {
		return m
	}
	m.cursor = index
	for i := range m.tabs {
		m.tabs[i].Active = i == index
	}
	return m
}

// PinIndex toggles the pinned state of the tab at index.
func (m Model) PinIndex(index int) Model {
	if index < 0 || index >= len(m.tabs) {
		return m
	}
	m.tabs[index].Pinned = !m.tabs[index].Pinned
	return m
}

// OpenFile adds or activates a tab for the given path.
func (m Model) OpenFile(path string) Model {
	for i := range m.tabs {
		m.tabs[i].Active = m.tabs[i].Path == path
		if m.tabs[i].Path == path {
			m.cursor = i
			return m
		}
	}
	// Not found — add new tab
	for i := range m.tabs {
		m.tabs[i].Active = false
	}
	m.tabs = append(m.tabs, Tab{
		Path:   path,
		Name:   tabName(path),
		Active: true,
	})
	m.cursor = len(m.tabs) - 1
	return m
}

// CloseFile removes the tab for the given path.
func (m Model) CloseFile(path string) Model {
	for i, t := range m.tabs {
		if t.Path == path {
			m.tabs = append(m.tabs[:i], m.tabs[i+1:]...)
			if m.cursor >= len(m.tabs) && m.cursor > 0 {
				m.cursor = len(m.tabs) - 1
			}
			break
		}
	}
	return m
}

// RenameFile updates the path and display name for the tab matching oldPath.
// If no tab matches, the model is returned unchanged.
func (m Model) RenameFile(oldPath, newPath string) Model {
	for i := range m.tabs {
		if m.tabs[i].Path == oldPath {
			m.tabs[i].Path = newPath
			m.tabs[i].Name = tabName(newPath)
			return m
		}
	}
	return m
}

// MarkDirty sets the dirty indicator on the tab matching path.
func (m Model) MarkDirty(path string) Model {
	for i := range m.tabs {
		if m.tabs[i].Path == path {
			m.tabs[i].Dirty = true
			break
		}
	}
	return m
}

// MarkClean clears the dirty indicator on the tab matching path.
func (m Model) MarkClean(path string) Model {
	for i := range m.tabs {
		if m.tabs[i].Path == path {
			m.tabs[i].Dirty = false
			break
		}
	}
	return m
}

// NextPath returns the path of the tab that would become active after
// closing the given path, or "" if no tabs would remain.
func (m Model) NextPath(closePath string) string {
	idx := -1
	for i, t := range m.tabs {
		if t.Path == closePath {
			idx = i
			break
		}
	}
	if idx < 0 {
		return ""
	}
	if len(m.tabs) <= 1 {
		return ""
	}
	if idx < len(m.tabs)-1 {
		return m.tabs[idx+1].Path
	}
	return m.tabs[idx-1].Path
}

func (m Model) Init() tea.Cmd { return nil }

func (m Model) Update(msg tea.Msg) (Model, tea.Cmd) {
	switch msg := msg.(type) {
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
		case key.Matches(msg, m.keys.PrimaryAction):
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

		b.WriteString(fmt.Sprintf("%d:", i+1))
		if t.Dirty {
			b.WriteString(m.styles.TabDirty.Render("x"))
		} else {
			b.WriteByte(' ')
		}
		b.WriteByte(' ')

		name := t.Name
		if t.Pinned {
			name = m.styles.TabPinned.Render("★") + " " + name
		}

		if t.Active {
			b.WriteString(m.styles.TabActive.Render(name))
		} else {
			b.WriteString(m.styles.TabNormal.Render(name))
		}
	}

	return lipgloss.NewStyle().
		MaxWidth(m.width).
		MaxHeight(m.height).
		Render(b.String())
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
