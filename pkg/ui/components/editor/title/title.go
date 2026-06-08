package title

import (
	"time"
	"unicode/utf8"

	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"

	"rune/pkg/ui/styles"
)

// RenameRequestMsg is emitted after the debounce timer fires, signalling
// that the user's title edit should trigger a file rename.
type RenameRequestMsg struct{ Name string }

// FocusReturnMsg is emitted when the user navigates away from the title
// (Down arrow or Enter), returning focus to the editor content.
type FocusReturnMsg struct{}

// debounceExpiredMsg is an internal timer message for rename debouncing.
type debounceExpiredMsg struct{ id int }

type Model struct {
	text        string
	cursor      int
	focused     bool
	placeholder string
	committed   string // last committed (saved/renamed) value for revert on Escape
	width       int
	styles      styles.Styles

	debounceID int
}

func New(placeholder string, st styles.Styles) Model {
	return Model{
		text:        placeholder,
		committed:   placeholder,
		placeholder: placeholder,
		styles:      st,
	}
}

func (m Model) Init() tea.Cmd  { return nil }
func (m Model) Height() int    { return 1 }
func (m Model) Text() string   { return m.text }
func (m Model) Focused() bool  { return m.focused }

// IsPlaceholder reports whether the title is still the original placeholder.
func (m Model) IsPlaceholder() bool { return m.text == m.placeholder }

func (m Model) SetSize(w, _ int) Model { m.width = w; return m }

func (m Model) SetText(name string) Model {
	m.text = name
	m.committed = name
	m.cursor = utf8.RuneCountInString(name)
	return m
}

func (m Model) SetFocused(v bool) Model {
	m.focused = v
	if v {
		m.cursor = utf8.RuneCountInString(m.text)
	}
	return m
}

func (m Model) Update(msg tea.Msg) (Model, tea.Cmd) {
	switch msg := msg.(type) {
	case debounceExpiredMsg:
		if msg.id == m.debounceID && m.text != m.committed {
			name := m.text
			m.committed = name
			return m, func() tea.Msg { return RenameRequestMsg{Name: name} }
		}
		return m, nil

	case tea.KeyPressMsg:
		if !m.focused {
			return m, nil
		}
		return m.handleKey(msg)
	}
	return m, nil
}

func (m Model) handleKey(msg tea.KeyPressMsg) (Model, tea.Cmd) {
	switch {
	// Down / Enter — return focus to editor content
	case msg.Code == tea.KeyDown && msg.Mod == 0,
		msg.Code == tea.KeyEnter && msg.Mod == 0:
		m.focused = false
		return m, func() tea.Msg { return FocusReturnMsg{} }

	// Escape — revert to committed value and unfocus
	case msg.Code == tea.KeyEscape && msg.Mod == 0:
		m.text = m.committed
		m.cursor = utf8.RuneCountInString(m.text)
		m.focused = false
		return m, nil

	// Backspace
	case msg.Code == tea.KeyBackspace && msg.Mod == 0:
		if m.cursor > 0 {
			runes := []rune(m.text)
			runes = append(runes[:m.cursor-1], runes[m.cursor:]...)
			m.text = string(runes)
			m.cursor--
			return m, m.startDebounce()
		}
		return m, nil

	// Delete
	case msg.Code == tea.KeyDelete && msg.Mod == 0:
		runes := []rune(m.text)
		if m.cursor < len(runes) {
			runes = append(runes[:m.cursor], runes[m.cursor+1:]...)
			m.text = string(runes)
			return m, m.startDebounce()
		}
		return m, nil

	// Left
	case msg.Code == tea.KeyLeft && msg.Mod == 0:
		if m.cursor > 0 {
			m.cursor--
		}
		return m, nil

	// Right
	case msg.Code == tea.KeyRight && msg.Mod == 0:
		if m.cursor < utf8.RuneCountInString(m.text) {
			m.cursor++
		}
		return m, nil

	// Home / Ctrl+A
	case msg.Code == tea.KeyHome && msg.Mod == 0,
		msg.Code == 'a' && msg.Mod == tea.ModCtrl:
		m.cursor = 0
		return m, nil

	// End / Ctrl+E
	case msg.Code == tea.KeyEnd && msg.Mod == 0,
		msg.Code == 'e' && msg.Mod == tea.ModCtrl:
		m.cursor = utf8.RuneCountInString(m.text)
		return m, nil

	// Up — do nothing (already at top)
	case msg.Code == tea.KeyUp && msg.Mod == 0:
		return m, nil

	default:
		// Printable text input
		text := msg.Text
		if text == "" && msg.Mod == 0 {
			code := msg.BaseCode
			if code == 0 {
				code = msg.Code
			}
			if code >= 32 && code < 127 {
				text = string(rune(code))
			}
		}
		if text != "" {
			runes := []rune(m.text)
			runes = append(runes[:m.cursor], append([]rune(text), runes[m.cursor:]...)...)
			m.text = string(runes)
			m.cursor += utf8.RuneCountInString(text)
			return m, m.startDebounce()
		}
	}
	return m, nil
}

func (m *Model) startDebounce() tea.Cmd {
	m.debounceID++
	id := m.debounceID
	return func() tea.Msg {
		time.Sleep(500 * time.Millisecond)
		return debounceExpiredMsg{id: id}
	}
}

func (m Model) View() string {
	titleStyle := lipgloss.NewStyle().
		Foreground(lipgloss.Color("216")).
		Bold(true).
		Padding(0, 1)

	if !m.focused {
		content := titleStyle.Render(m.text)
		return lipgloss.NewStyle().MaxWidth(m.width).Render(content)
	}

	// Render as a single styled run; cursor char highlighted via inline reverse.
	// The cursor's ANSI Reset would wipe the orange foreground, so we re-apply
	// the title style to the text after the cursor.
	runes := []rune(m.text)
	var before, after string
	var cursorChar string

	if m.cursor < len(runes) {
		before = string(runes[:m.cursor])
		cursorChar = string(runes[m.cursor])
		after = string(runes[m.cursor+1:])
	} else {
		before = m.text
		cursorChar = " "
		after = ""
	}

	cursorPart := lipgloss.NewStyle().Reverse(true).Render(cursorChar)
	afterPart := lipgloss.NewStyle().
		Foreground(lipgloss.Color("216")).
		Bold(true).
		Render(after)

	content := before + cursorPart + afterPart

	return lipgloss.NewStyle().
		Foreground(lipgloss.Color("216")).
		Bold(true).
		Padding(0, 1).
		MaxWidth(m.width).
		Render(content)
}
