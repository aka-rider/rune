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
// keyboard navigation or mouse click within the focused opentabs component.
type TabSelectedMsg struct {
	DocID int64
	Path  string
}

// TabHandle identifies a tab by its stable VFS document identity.
type TabHandle struct {
	DocID int64
	Path  string
}

type Tab struct {
	DocID  int64
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
	offsetX int
	offsetY int
	focused bool
	keys    keymap.Bindings
	styles  styles.Styles
}

func New(keys keymap.Bindings, st styles.Styles) Model {
	return Model{keys: keys, styles: st}
}

func (m Model) SetSize(w, h int) Model   { m.width = w; m.height = h; return m }
func (m Model) SetOffset(x, y int) Model { m.offsetX = x; m.offsetY = y; return m }
func (m Model) SetFocused(f bool) Model  { m.focused = f; return m }
func (m Model) Focused() bool            { return m.focused }
func (m Model) Cursor() int              { return m.cursor }
func (m Model) Len() int                 { return len(m.tabs) }

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

// DocIDAt returns the VFS document ID at the given tab index, or 0 if OOB.
func (m Model) DocIDAt(index int) int64 {
	if index < 0 || index >= len(m.tabs) {
		return 0
	}
	return m.tabs[index].DocID
}

// SelectIndex switches the active tab to the given index.
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

// SelectByID activates the tab with the given docID. Returns unchanged if not found.
func (m Model) SelectByID(docID int64) Model {
	for i, t := range m.tabs {
		if t.DocID == docID {
			return m.SelectIndex(i)
		}
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

// OpenFile adds or activates a tab for the given docID+path pair.
// When docID != 0, the tab is keyed by docID (rename-safe).
// When docID == 0, falls back to path-keying for virtual docs (help, initial
// untitled) where no VFS identity has been assigned yet.
func (m Model) OpenFile(docID int64, path string) Model {
	if docID != 0 {
		for i, t := range m.tabs {
			if t.DocID == docID {
				m.tabs[i].Path = path
				m.tabs[i].Name = tabName(path)
				return m.SelectIndex(i)
			}
		}
	} else {
		for i, t := range m.tabs {
			if t.DocID == 0 && t.Path == path {
				return m.SelectIndex(i)
			}
		}
	}
	// Not found — add new tab.
	for i := range m.tabs {
		m.tabs[i].Active = false
	}
	m.tabs = append(m.tabs, Tab{
		DocID:  docID,
		Path:   path,
		Name:   tabName(path),
		Active: true,
	})
	m.cursor = len(m.tabs) - 1
	return m
}

// CloseByID removes the tab with the given docID.
func (m Model) CloseByID(docID int64) Model {
	for i, t := range m.tabs {
		if t.DocID == docID {
			m.tabs = append(m.tabs[:i], m.tabs[i+1:]...)
			if m.cursor >= len(m.tabs) && m.cursor > 0 {
				m.cursor = len(m.tabs) - 1
			}
			break
		}
	}
	return m
}

// CloseFile removes the tab for the given path. Prefer CloseByID when the
// docID is known.
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

// RenameFile updates path and display name for the tab matching oldPath.
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

// SetTabNameByID overrides the display name of the tab with the given docID.
func (m Model) SetTabNameByID(docID int64, name string) Model {
	for i := range m.tabs {
		if m.tabs[i].DocID == docID {
			m.tabs[i].Name = name
			return m
		}
	}
	return m
}

// SetTabName overrides the display name of the tab matching path.
// Prefer SetTabNameByID when the docID is known.
func (m Model) SetTabName(path, name string) Model {
	for i := range m.tabs {
		if m.tabs[i].Path == path {
			m.tabs[i].Name = name
			return m
		}
	}
	return m
}

// MarkDirtyByID sets the dirty indicator on the tab with the given docID.
func (m Model) MarkDirtyByID(docID int64) Model {
	for i := range m.tabs {
		if m.tabs[i].DocID == docID {
			m.tabs[i].Dirty = true
			break
		}
	}
	return m
}

// MarkCleanByID clears the dirty indicator on the tab with the given docID.
func (m Model) MarkCleanByID(docID int64) Model {
	for i := range m.tabs {
		if m.tabs[i].DocID == docID {
			m.tabs[i].Dirty = false
			break
		}
	}
	return m
}

// MarkDirty sets the dirty indicator on the tab matching path.
// Prefer MarkDirtyByID when the docID is known.
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
// Prefer MarkCleanByID when the docID is known.
func (m Model) MarkClean(path string) Model {
	for i := range m.tabs {
		if m.tabs[i].Path == path {
			m.tabs[i].Dirty = false
			break
		}
	}
	return m
}

// HasDirty reports whether any open tab has unsaved changes.
func (m Model) HasDirty() bool {
	for _, t := range m.tabs {
		if t.Dirty {
			return true
		}
	}
	return false
}

// DirtyTabs returns handles for all tabs that have unsaved changes.
func (m Model) DirtyTabs() []TabHandle {
	var out []TabHandle
	for _, t := range m.tabs {
		if t.Dirty {
			out = append(out, TabHandle{DocID: t.DocID, Path: t.Path})
		}
	}
	return out
}

// NextDocID returns the docID of the tab that would become active after
// closing the given docID, or 0 if no tabs would remain.
func (m Model) NextDocID(closeDocID int64) int64 {
	idx := -1
	for i, t := range m.tabs {
		if t.DocID == closeDocID {
			idx = i
			break
		}
	}
	if idx < 0 || len(m.tabs) <= 1 {
		return 0
	}
	if idx < len(m.tabs)-1 {
		return m.tabs[idx+1].DocID
	}
	return m.tabs[idx-1].DocID
}

// NextPath returns the path of the tab that would become active after
// closing the given path, or "" if no tabs would remain.
// Prefer NextDocID when the docID is known.
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
			t := m.tabs[m.cursor]
			return m, func() tea.Msg { return TabSelectedMsg{DocID: t.DocID, Path: t.Path} }
		}
	case tea.MouseClickMsg:
		return m.handleMouseClick(msg)
	}
	return m, nil
}

// handleMouseClick selects the tab under the click and asks the page to open
// it. Row 0 of the component is the "── Open ──" header; tabs start at row 1.
func (m Model) handleMouseClick(msg tea.MouseClickMsg) (Model, tea.Cmd) {
	if !m.focused || msg.Button != tea.MouseLeft {
		return m, nil
	}
	idx := msg.Y - m.offsetY - 1
	if idx < 0 || idx >= len(m.tabs) {
		return m, nil
	}
	m = m.SelectIndex(idx)
	t := m.tabs[idx]
	return m, func() tea.Msg { return TabSelectedMsg{DocID: t.DocID, Path: t.Path} }
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
