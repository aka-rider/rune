package filetree

import (
	"strings"
	"time"

	"charm.land/bubbles/v2/key"
	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"

	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"
)

type FileSelectedMsg struct{ Path string }
type DirSelectedMsg struct{ Path string }

type DirLoadedMsg struct {
	Root    string
	Entries []Entry
}

// DirReloadedMsg is identical to DirLoadedMsg but signals a disk-triggered
// reload (fsnotify) rather than user navigation. The filetree uses this to
// decide whether to preserve or reset the cursor.
type DirReloadedMsg struct {
	Root    string
	Entries []Entry
}

type Model struct {
	entries      []Entry
	cursor       int
	root         string
	width        int
	height       int
	offsetX      int
	offsetY      int
	focused      bool
	keys         keymap.Bindings
	styles       styles.Styles
	searchQuery  string
	lastLetterAt time.Time
}

func New(keys keymap.Bindings, st styles.Styles) Model {
	return Model{keys: keys, styles: st}
}

func (m Model) SetSize(w, h int) Model   { m.width = w; m.height = h; return m }
func (m Model) SetOffset(x, y int) Model { m.offsetX = x; m.offsetY = y; return m }
func (m Model) SetFocused(f bool) Model  { m.focused = f; return m }
func (m Model) Height() int              { return m.height }
func (m Model) Root() string             { return m.root }

func (m Model) Init() tea.Cmd { return nil }

func (m Model) Update(msg tea.Msg) (Model, tea.Cmd) {
	switch msg := msg.(type) {
	case DirLoadedMsg:
		// User navigation — reset cursor to top.
		m = m.handleDirLoad(msg.Entries, msg.Root, true)

	case DirReloadedMsg:
		// Disk-triggered reload — preserve cursor position.
		m = m.handleDirLoad(msg.Entries, msg.Root, false)
	case tea.MouseClickMsg:
		return m.handleMouseClick(msg)
	case tea.MouseWheelMsg:
		return m.handleMouseWheel(msg)
	case tea.KeyPressMsg:
		if !m.focused {
			break
		}

		switch {
		case key.Matches(msg, m.keys.Up):
			if m.cursor > 0 {
				m.cursor--
			}
			return m, nil
		case key.Matches(msg, m.keys.Down):
			if m.cursor < len(m.entries)-1 {
				m.cursor++
			}
			return m, nil
		case key.Matches(msg, m.keys.GotoTop):
			m.cursor = 0
			return m, nil
		case key.Matches(msg, m.keys.GotoBottom):
			if len(m.entries) > 0 {
				m.cursor = len(m.entries) - 1
			}
			return m, nil
		case key.Matches(msg, m.keys.PrimaryAction):
			if len(m.entries) > 0 {
				e := m.entries[m.cursor]
				if e.IsDir {
					return m, func() tea.Msg { return DirSelectedMsg{Path: e.Path} }
				}
				return m, func() tea.Msg { return FileSelectedMsg{Path: e.Path} }
			}
			return m, nil
		}

		if msg.Text != "" {
			if time.Since(m.lastLetterAt) > 750*time.Millisecond {
				m.searchQuery = ""
			}
			m.searchQuery += msg.Text
			m.lastLetterAt = time.Now()

			lowerQuery := strings.ToLower(m.searchQuery)
			for i, e := range m.entries {
				if strings.HasPrefix(strings.ToLower(e.Name), lowerQuery) {
					m.cursor = i
					break
				}
			}
			return m, nil
		}
	}
	return m, nil
}

// handleDirLoad applies a directory load or reload, assigning entries and root,
// and either resetting the cursor to 0 (navigation) or preserving it (reload).
func (m Model) handleDirLoad(entries []Entry, root string, resetCursor bool) Model {
	// Capture the previous entry name from the old entries before replacing.
	var prevName string
	if len(m.entries) > 0 && m.cursor < len(m.entries) {
		prevName = m.entries[m.cursor].Name
	}
	oldCursor := m.cursor

	m.entries = entries
	m.root = root

	if resetCursor {
		m.cursor = 0
		return m
	}

	// Preserve cursor: try name match, then fall back to old index.
	if len(entries) == 0 {
		m.cursor = 0
		return m
	}

	if prevName != "" {
		for i, e := range entries {
			if e.Name == prevName {
				m.cursor = i
				return m
			}
		}
	}

	// Name not found — clamp old index.
	if oldCursor < len(entries) {
		m.cursor = oldCursor
	} else {
		m.cursor = len(entries) - 1
	}
	return m
}

func (m Model) View() string {
	content := renderFileList(m)
	return lipgloss.NewStyle().
		MaxWidth(m.width).
		MaxHeight(m.height).
		Render(content)
}

func renderFileList(m Model) string {
	var b strings.Builder

	root := m.root
	if root == "" {
		root = "."
	}
	b.WriteString(m.styles.PaneTitle.Render(root))

	if len(m.entries) == 0 {
		return b.String()
	}

	maxVisible := m.height - 1
	if maxVisible <= 0 {
		return b.String()
	}

	start := 0
	if m.cursor >= maxVisible {
		start = m.cursor - maxVisible + 1
	}
	end := start + maxVisible
	if end > len(m.entries) {
		end = len(m.entries)
	}

	for i := start; i < end; i++ {
		e := m.entries[i]
		name := e.Name
		if e.IsDir {
			name += "/"
		}

		b.WriteByte('\n')
		if i == m.cursor {
			b.WriteString("> ")
			b.WriteString(m.styles.FileSelected.Render(name))
		} else {
			b.WriteString("  ")
			b.WriteString(m.styles.FileNormal.Render(name))
		}
	}

	return b.String()
}
