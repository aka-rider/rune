package dictation

import (
	"strings"

	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"

	"rune/pkg/ui/styles"
)

// Model is the visual feedback panel shown during a dictation session.
// Height() returns 0 when hidden and 3 when visible, enabling the workspace
// to subtract it from contentH via recalcLayout().
type Model struct {
	visible bool
	text    string // accumulated partial transcription
	errMsg  string // non-empty when a transcription error occurred
	width   int
	styles  styles.Styles
}

// New creates a hidden dictation panel.
func New(st styles.Styles) Model {
	return Model{styles: st}
}

func (m Model) Init() tea.Cmd { return nil }

func (m Model) Update(msg tea.Msg) (Model, tea.Cmd) {
	if ws, ok := msg.(tea.WindowSizeMsg); ok {
		m.width = ws.Width
	}
	return m, nil
}

// SetSize stores the allocated width (height is always 3 when visible).
func (m Model) SetSize(w, _ int) Model { m.width = w; return m }

// Height returns 3 when visible, 0 when hidden.
func (m Model) Height() int {
	if m.visible {
		return 3
	}
	return 0
}

func (m Model) SetVisible(v bool) Model  { m.visible = v; return m }
func (m Model) SetText(t string) Model   { m.text = t; return m }
func (m Model) SetError(err error) Model { m.errMsg = err.Error(); return m }

// ClearAll resets text and error state, ready for a new session.
func (m Model) ClearAll() Model {
	m.text = ""
	m.errMsg = ""
	return m
}

// View renders a 3-line bordered panel or "" when hidden.
//
//	┌─ 🎤 Listening ──────────────────────────────────┐
//	│ <accumulated text or error>                      │
//	└─ ^v to stop ────────────────────────────────────┘
func (m Model) View() string {
	if !m.visible || m.width == 0 {
		return ""
	}

	const (
		topPrefix = "┌─ 🎤 Listening "
		topSuffix = "┐"
		botPrefix = "└─ ^v to stop "
		botSuffix = "┘"
		leftEdge  = "│ "
		rightEdge = " │"
	)

	topFill := m.width - lipgloss.Width(topPrefix) - lipgloss.Width(topSuffix)
	if topFill < 0 {
		topFill = 0
	}
	top := topPrefix + strings.Repeat("─", topFill) + topSuffix

	botFill := m.width - lipgloss.Width(botPrefix) - lipgloss.Width(botSuffix)
	if botFill < 0 {
		botFill = 0
	}
	bot := botPrefix + strings.Repeat("─", botFill) + botSuffix

	innerW := m.width - lipgloss.Width(leftEdge) - lipgloss.Width(rightEdge)
	if innerW < 0 {
		innerW = 0
	}

	var content string
	if m.errMsg != "" {
		content = m.styles.Error.Render(m.errMsg)
	} else {
		content = m.text
	}

	contentW := lipgloss.Width(content)
	padRight := innerW - contentW
	if padRight < 0 {
		padRight = 0
	}
	mid := leftEdge + content + strings.Repeat(" ", padRight) + rightEdge

	return strings.Join([]string{top, mid, bot}, "\n")
}
