package footer

import (
	"fmt"
	"strings"

	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"

	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"
)

type UpdateCursorMsg struct {
	Line      int
	Col       int
	WordCount int
}

type Model struct {
	line      int
	col       int
	wordCount int
	width     int
	keys      keymap.Bindings
	styles    styles.Styles
}

func New(keys keymap.Bindings, st styles.Styles) Model {
	return Model{keys: keys, styles: st}
}

func (m Model) SetSize(w, h int) Model { m.width = w; return m }
func (m Model) Height() int            { return 1 }

func (m Model) Init() tea.Cmd { return nil }

func (m Model) Update(msg tea.Msg) (Model, tea.Cmd) {
	if msg, ok := msg.(UpdateCursorMsg); ok {
		m.line = msg.Line
		m.col = msg.Col
		m.wordCount = msg.WordCount
	}
	return m, nil
}

func (m Model) View() string {
	left := m.styles.FooterKey.Render("^1") +
		m.styles.FooterHint.Render(" left  ") +
		m.styles.FooterKey.Render("^2") +
		m.styles.FooterHint.Render(" editor  ") +
		m.styles.FooterKey.Render("esc") +
		m.styles.FooterHint.Render(" zen  ") +
		m.styles.FooterKey.Render("q") +
		m.styles.FooterHint.Render(" quit")

	right := m.styles.FooterMeta.Render(
		fmt.Sprintf("Ln %d, Col %d  W:%d  🎤 Off", m.line+1, m.col+1, m.wordCount),
	)

	// -2 accounts for the Padding(0,1) on the Footer style (1 cell each side)
	innerWidth := m.width - 2
	gap := innerWidth - lipgloss.Width(left) - lipgloss.Width(right)
	if gap < 1 {
		gap = 1
	}
	content := left + strings.Repeat(" ", gap) + right

	return m.styles.Footer.Width(m.width).Render(content)
}
