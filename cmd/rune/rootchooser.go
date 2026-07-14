package main

import (
	"fmt"
	"strings"

	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"

	"rune/pkg/ui/styles"
	"rune/pkg/workspaceroot"
)

// rootChooser is a small, self-contained tea.Model that asks the user where
// to create a NEW rune workspace (.rune/) when workspaceroot.Resolve found no
// existing one walking up from the launch directory. It runs to completion
// in its own tea.Program *before* the main app (see cmd/rune/main.go) and
// hands back a fully-resolved absolute directory exactly like -w would.
//
// Init/Update/View all use value receivers and View is a pure function of
// Model state (§5.1/§5.2) — this is an ordinary Elm-cycle component, just one
// that happens to drive its own top-level tea.Program instead of being wired
// into the main app's router.
type rootChooser struct {
	candidates []workspaceroot.Candidate
	cursor     int
	width      int
	height     int
	styles     styles.Styles

	// Terminal state after the program exits: exactly one of quit/decided is
	// true (§1.7 — no sentinel abuse via an empty-string convention). The
	// full Candidate is kept (not just Dir) so a KindMemory pick — which has
	// no real disk location to speak of — doesn't need an empty-string or
	// ":memory:" sentinel to signal itself.
	quit      bool
	decided   bool
	candidate workspaceroot.Candidate
}

// newRootChooser builds the chooser from an undecided Resolve prompt. The
// initial cursor is prompt.Default, clamped defensively in case a future
// caller ever hands in an out-of-range value.
func newRootChooser(prompt *workspaceroot.Prompt, st styles.Styles) rootChooser {
	cursor := prompt.Default
	if cursor < 0 || cursor >= len(prompt.Candidates) {
		cursor = 0
	}
	return rootChooser{
		candidates: prompt.Candidates,
		cursor:     cursor,
		styles:     st,
	}
}

func (m rootChooser) Init() tea.Cmd { return nil }

func (m rootChooser) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	switch msg := msg.(type) {
	case tea.WindowSizeMsg:
		m.width = msg.Width
		m.height = msg.Height
		return m, nil

	case tea.KeyPressMsg:
		return m.handleKey(msg)
	}
	return m, nil
}

func (m rootChooser) handleKey(msg tea.KeyPressMsg) (tea.Model, tea.Cmd) {
	switch {
	case msg.Code == tea.KeyUp && msg.Mod == 0:
		if m.cursor > 0 {
			m.cursor--
		}
		return m, nil

	case msg.Code == tea.KeyDown && msg.Mod == 0:
		if m.cursor < len(m.candidates)-1 {
			m.cursor++
		}
		return m, nil

	case msg.Code == tea.KeyEnter && msg.Mod == 0:
		m.decided = true
		m.candidate = m.candidates[m.cursor]
		return m, tea.Quit

	case msg.Code == tea.KeyEscape && msg.Mod == 0:
		m.quit = true
		return m, tea.Quit

	case msg.Code == 'c' && msg.Mod == tea.ModCtrl:
		m.quit = true
		return m, tea.Quit
	}
	return m, nil
}

// Chosen reports the picked candidate. ok is false if the user quit
// (Esc/Ctrl+C) without picking anything.
func (m rootChooser) Chosen() (candidate workspaceroot.Candidate, ok bool) {
	return m.candidate, m.decided
}

func (m rootChooser) View() tea.View {
	return tea.NewView(m.render())
}

// render is the pure string-rendering half of View, split out so tests can
// assert on plain output without threading tea.View's extra fields.
func (m rootChooser) render() string {
	var b strings.Builder
	b.WriteString(m.styles.PaneTitle.Render("Create workspace where?"))
	b.WriteString("\n\n")

	for i, c := range m.candidates {
		line := m.renderCandidate(c, i == m.cursor)
		b.WriteString(line)
		b.WriteString("\n")
	}

	b.WriteString("\n")
	b.WriteString(m.styles.FooterHint.Render("↑/↓ move · enter select · esc quit"))

	box := m.styles.ActiveBorder.Padding(1, 3).Render(b.String())

	if m.width <= 0 || m.height <= 0 {
		return box
	}
	return lipgloss.Place(m.width, m.height, lipgloss.Center, lipgloss.Center, box)
}

// renderCandidate renders one candidate line: cursor indicator, the
// directory path prominent, then a dimmed "/.rune" suffix and kind hint.
// KindMemory has no disk location to show, so it renders as a plain "None"
// label with a suffix explaining the consequence instead.
func (m rootChooser) renderCandidate(c workspaceroot.Candidate, selected bool) string {
	pointer := "  "
	dirStyle := m.styles.FileNormal
	if selected {
		pointer = "> "
		dirStyle = m.styles.FileSelected
	}
	if c.Kind == workspaceroot.KindMemory {
		label := dirStyle.Render("None")
		suffix := m.styles.DirSuffix.Render(" (in-memory, nothing saved)")
		return pointer + label + suffix
	}
	dir := dirStyle.Render(c.Dir)
	suffix := m.styles.DirSuffix.Render(fmt.Sprintf("/.rune (%s)", c.Kind))
	return pointer + dir + suffix
}
