// dbg_keyboard prints key event details for layout-independent key debugging.
//
// Run it and press keys to see how the terminal + ultraviolet decode them.
// With Kitty keyboard enhancement flags enabled, BaseCode gives the physical
// key position (PC-101 scancode) which is layout-independent.
package main

import (
	"fmt"
	"os"
	"strings"

	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"
)

func main() {
	m := model{}
	if _, err := tea.NewProgram(m).Run(); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}

type model struct {
	events []keyEvent
	width  int
	height int
}

type keyEvent struct {
	Code        rune
	BaseCode    rune
	ShiftedCode rune
	Mod         tea.KeyMod
	Text        string
	KeyStr      string
	Keystroke   string
}

func (m model) Init() tea.Cmd {
	return nil
}

func (m model) View() tea.View {
	v := tea.NewView("")
	v.AltScreen = true
	v.KeyboardEnhancements.ReportAllKeysAsEscapeCodes = true
	v.KeyboardEnhancements.ReportAlternateKeys = true

	headerStyle := lipgloss.NewStyle().Bold(true).Foreground(lipgloss.Color("205"))
	labelStyle := lipgloss.NewStyle().Foreground(lipgloss.Color("63"))
	bodyStyle := lipgloss.NewStyle().Foreground(lipgloss.Color("243"))

	var lines []string
	lines = append(lines, "dbg_keyboard — press keys to inspect")
	lines = append(lines, "")
	lines = append(lines, headerStyle.Render("Field")+"\t"+labelStyle.Render("Meaning"))
	lines = append(lines, strings.Repeat("-", 60))
	lines = append(lines, fmt.Sprintf("%-12s\t%s", "Code", "unicode codepoint of character produced (layout-DEPENDENT)"))
	lines = append(lines, fmt.Sprintf("%-12s\t%s", "BaseCode", "physical key position via Kitty protocol (layout-INDEPENDENT)"))
	lines = append(lines, fmt.Sprintf("%-12s\t%s", "ShiftedCode", "BaseCode with shift applied"))
	lines = append(lines, fmt.Sprintf("%-12s\t%s", "Text", "shifted character from BaseCode"))
	lines = append(lines, fmt.Sprintf("%-12s\t%s", "Mod", "modifier flags"))
	lines = append(lines, fmt.Sprintf("%-12s\t%s", "Keystroke()", "key.String() after modifier stripping"))
	lines = append(lines, "")

	if len(m.events) == 0 {
		lines = append(lines, bodyStyle.Render("  (no keys pressed yet)"))
	} else {
		start := len(m.events) - 25
		if start < 0 {
			start = 0
		}
		for _, e := range m.events[start:] {
			lines = append(lines, bodyStyle.Render(fmt.Sprintf(
				"Code=%d(%c) BaseCode=%d(%c) ShiftedCode=%d(%c) Mod=%d Text=%q KeyStr=%q Keystroke=%q",
				e.Code, e.Code,
				e.BaseCode, e.BaseCode,
				e.ShiftedCode, e.ShiftedCode,
				e.Mod,
				e.Text,
				e.KeyStr,
				e.Keystroke,
			)))
		}
	}

	content := lipgloss.NewStyle().
		MaxWidth(m.width).
		MaxHeight(m.height).
		Render(lipgloss.JoinVertical(lipgloss.Left, lines...))

	v.Content = content
	return v
}

func (m model) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	switch msg := msg.(type) {
	case tea.WindowSizeMsg:
		m.width = msg.Width
		m.height = msg.Height
		return m, nil

	case tea.KeyPressMsg:
		ke := keyEvent{
			Code:        msg.Code,
			BaseCode:    msg.BaseCode,
			ShiftedCode: msg.ShiftedCode,
			Mod:         msg.Mod,
			Text:        msg.Text,
			KeyStr:      msg.String(),
			Keystroke:   msg.Key().String(),
		}
		m.events = append(m.events, ke)
		if len(m.events) > 100 {
			m.events = m.events[len(m.events)-100:]
		}
		return m, nil
	}
	return m, nil
}
