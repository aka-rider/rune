package footer

import (
	"fmt"
	"strings"
	"time"

	"charm.land/bubbles/v2/key"
	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"

	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"
)

// Entry is a key+description pair for footer help display.
// Re-exported alias so the footer doesn't need to import keymap.
type Entry = keymap.HelpEntry

// ConfirmQuitMsg is emitted when a chord exit sequence completes (e.g., ^C^C).
type ConfirmQuitMsg struct{}

// DirtyGuardResponse enumerates user responses to a dirty guard prompt.
type DirtyGuardResponse int

const (
	DirtyGuardSave DirtyGuardResponse = iota
	DirtyGuardDiscard
	DirtyGuardCancel
)

// DirtyGuardResponseMsg is emitted when the user responds to a dirty guard prompt.
type DirtyGuardResponseMsg struct {
	Response DirtyGuardResponse
}

// confirmExpired is an internal message to reset chord state after timeout.
type confirmExpired struct{}

// ShowErrorMsg tells the footer to display a transient error message.
type ShowErrorMsg struct{ Text string }

// errorDismissedMsg is an internal timer message to clear the error after timeout.
type errorDismissedMsg struct{ id int }

type UpdateCursorMsg struct {
	Line      int
	Col       int
	WordCount int
}

type Model struct {
	line             int
	col              int
	wordCount        int
	width            int
	styles           styles.Styles
	keys             keymap.Bindings
	pendingKey       string
	helpExpanded     bool
	helpEntries      []Entry
	dirtyGuard       bool
	dictating        bool
	dictationAllowed bool
	errorMsg         string
	errorExpireID    int
}

// DictationStartMsg is emitted when the user activates voice dictation (^v).
type DictationStartMsg struct{}

// DictationStopMsg is emitted when the user stops voice dictation (^v again).
type DictationStopMsg struct{}

func New(keys keymap.Bindings, st styles.Styles) Model {
	return Model{keys: keys, styles: st}
}

func (m Model) SetSize(w, h int) Model              { m.width = w; return m }
func (m Model) SetHelp(e []Entry) Model             { m.helpEntries = e; return m }
func (m Model) SetHelpExpanded(expanded bool) Model { m.helpExpanded = expanded; return m }
func (m Model) HelpExpanded() bool                  { return m.helpExpanded }
func (m Model) Height() int                         { return 1 }
func (m Model) SetDirtyGuard(active bool) Model     { m.dirtyGuard = active; return m }
func (m Model) InDirtyGuard() bool                  { return m.dirtyGuard }

func (m Model) SetDictationAllowed(allowed bool) Model { m.dictationAllowed = allowed; return m }
func (m Model) SetDictating(active bool) Model         { m.dictating = active; return m }
func (m Model) IsDictating() bool                      { return m.dictating }

func (m Model) Init() tea.Cmd { return nil }

func (m Model) Update(msg tea.Msg) (Model, tea.Cmd) {
	switch msg := msg.(type) {
	case tea.KeyPressMsg:
		// Dirty guard mode consumes all keypresses until resolved.
		if m.dirtyGuard {
			switch {
			case msg.Code == 's' && msg.Mod == 0:
				m.dirtyGuard = false
				return m, func() tea.Msg { return DirtyGuardResponseMsg{Response: DirtyGuardSave} }
			case msg.Code == 'd' && msg.Mod == 0:
				m.dirtyGuard = false
				return m, func() tea.Msg { return DirtyGuardResponseMsg{Response: DirtyGuardDiscard} }
			case key.Matches(msg, m.keys.Cancel):
				m.dirtyGuard = false
				return m, func() tea.Msg { return DirtyGuardResponseMsg{Response: DirtyGuardCancel} }
			}
			// Consume all other keys during guard mode.
			return m, nil
		}

		switch {
		case key.Matches(msg, m.keys.ConfirmExitC):
			if m.pendingKey == "c" {
				m.pendingKey = ""
				return m, func() tea.Msg { return ConfirmQuitMsg{} }
			}
			m.pendingKey = "c"
			return m, startConfirmTimer()

		case key.Matches(msg, m.keys.ConfirmExitD):
			if m.pendingKey == "d" {
				m.pendingKey = ""
				return m, func() tea.Msg { return ConfirmQuitMsg{} }
			}
			m.pendingKey = "d"
			return m, startConfirmTimer()

		case key.Matches(msg, m.keys.HelpExpand):
			m.helpExpanded = !m.helpExpanded

		case key.Matches(msg, m.keys.VoiceDictation):
			if !m.dictationAllowed {
				return m, nil
			}
			if m.dictating {
				m.dictating = false
				return m, func() tea.Msg { return DictationStopMsg{} }
			}
			m.dictating = true
			return m, func() tea.Msg { return DictationStartMsg{} }
		}

	case confirmExpired:
		m.pendingKey = ""

	case UpdateCursorMsg:
		m.line = msg.Line
		m.col = msg.Col
		m.wordCount = msg.WordCount

	case ShowErrorMsg:
		m.errorMsg = msg.Text
		m.errorExpireID++
		id := m.errorExpireID
		return m, func() tea.Msg {
			time.Sleep(5 * time.Second)
			return errorDismissedMsg{id: id}
		}

	case errorDismissedMsg:
		if msg.id == m.errorExpireID {
			m.errorMsg = ""
		}
	}
	return m, nil
}

func startConfirmTimer() tea.Cmd {
	return func() tea.Msg {
		time.Sleep(2 * time.Second)
		return confirmExpired{}
	}
}

func (m Model) View() string {
	// Error messages take precedence over normal status display.
	if m.errorMsg != "" {
		errContent := m.styles.Error.Render("⚠ " + m.errorMsg)
		return m.styles.Footer.Width(m.width).MaxHeight(1).Render(errContent)
	}

	var left string

	if m.dictating {
		left = m.styles.FooterKey.Render("^v") + m.styles.FooterHint.Render(" stop dictation")
	} else if m.dirtyGuard {
		left = m.styles.FooterKey.Render("Unsaved changes.") +
			m.styles.FooterHint.Render(" [") +
			m.styles.FooterKey.Render("S") +
			m.styles.FooterHint.Render("]ave [") +
			m.styles.FooterKey.Render("D") +
			m.styles.FooterHint.Render("]iscard [") +
			m.styles.FooterKey.Render("Esc") +
			m.styles.FooterHint.Render("] Cancel")
	} else if m.pendingKey == "c" {
		left = m.styles.FooterKey.Render("Press ^C again to exit")
	} else if m.pendingKey == "d" {
		left = m.styles.FooterKey.Render("Press ^D again to exit")
	} else if m.helpExpanded && len(m.helpEntries) > 0 {
		var parts []string
		for _, e := range m.helpEntries {
			parts = append(parts, m.styles.FooterKey.Render(e.Key)+m.styles.FooterHint.Render(" "+e.Desc))
		}
		left = strings.Join(parts, "  ")
	} else if len(m.helpEntries) > 0 {
		// Compact: show first 3 entries
		n := 3
		if len(m.helpEntries) < n {
			n = len(m.helpEntries)
		}
		var parts []string
		for _, e := range m.helpEntries[:n] {
			parts = append(parts, m.styles.FooterKey.Render(e.Key)+m.styles.FooterHint.Render(" "+e.Desc))
		}
		left = strings.Join(parts, "  ")
	}

	micIcon := m.styles.FooterMeta.Render("🎤")
	if m.dictationAllowed && m.dictating {
		micIcon = m.styles.FooterKey.Render("🎤 ●")
	}
	right := m.styles.FooterMeta.Render(
		fmt.Sprintf("Ln %d, Col %d  W:%d  ", m.line+1, m.col+1, m.wordCount),
	) + micIcon

	// -2 accounts for the Padding(0,1) on the Footer style (1 cell each side)
	innerWidth := m.width - 2
	gap := innerWidth - lipgloss.Width(left) - lipgloss.Width(right)
	if gap < 1 {
		gap = 1
	}
	content := left + strings.Repeat(" ", gap) + right

	return m.styles.Footer.Width(m.width).MaxHeight(1).Render(content)
}
