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

// ConfirmQuitMsg is emitted when a chord exit sequence completes (e.g., ^C^C).
type ConfirmQuitMsg struct{}

// GuardKind identifies which type of guard is active.
type GuardKind int

const (
	GuardDirty GuardKind = iota
	GuardMerge
)

// GuardOption maps a keyboard input to a guard response.
type GuardOption struct {
	Key      rune
	Response DataLossGuardResponse
}

// DataLossGuardResponse enumerates user responses to data-loss guard prompts.
type DataLossGuardResponse int

const (
	DataLossSave DataLossGuardResponse = iota
	DataLossDiscard
	DataLossCancel
	DataLossMergeAccept
	DataLossMergeReject
)

// DataLossGuardResponseMsg is emitted when the user responds to a guard prompt.
type DataLossGuardResponseMsg struct {
	Response DataLossGuardResponse
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
	guardKind        GuardKind
	guardOptions     []GuardOption
	guardLabel       string // custom label for the dirty guard (e.g. victim filename)
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

func (m Model) SetSize(w, h int) Model { m.width = w; return m }
func (m Model) Height() int            { return 1 }
func (m Model) SetGuard(kind GuardKind, options []GuardOption) Model {
	m.guardKind = kind
	m.guardOptions = options
	m.guardLabel = "" // reset so prior eviction labels never leak into close/quit guards
	return m
}

// SetGuardLabel overrides the label shown in the dirty guard prompt. An empty
// string (the default after SetGuard) renders the standard "Unsaved changes."
// message. Used to name the eviction victim ("Close %q — unsaved.").
func (m Model) SetGuardLabel(label string) Model { m.guardLabel = label; return m }

func (m Model) InGuard() bool        { return len(m.guardOptions) > 0 }
func (m Model) GuardKind() GuardKind { return m.guardKind }

func (m Model) SetDictationAllowed(allowed bool) Model { m.dictationAllowed = allowed; return m }
func (m Model) SetDictating(active bool) Model         { m.dictating = active; return m }
func (m Model) IsDictating() bool                      { return m.dictating }

func (m Model) Init() tea.Cmd { return nil }

func (m Model) Update(msg tea.Msg) (Model, tea.Cmd) {
	switch msg := msg.(type) {
	case tea.KeyPressMsg:
		// Guard mode consumes all keypresses until resolved.
		if len(m.guardOptions) > 0 {
			for _, opt := range m.guardOptions {
				if opt.Key != 0 && msg.Text == string(opt.Key) && msg.Mod == 0 {
					m.guardKind = 0
					m.guardOptions = nil
					m.guardLabel = ""
					return m, func() tea.Msg { return DataLossGuardResponseMsg{Response: opt.Response} }
				}
			}
			// Enter key maps to the first option (merge guard [Y]es only).
			// Dirty guard must NOT auto-resolve on Enter — users expect
			// Enter to be inert while the dirty prompt is active.
			if msg.Code == tea.KeyEnter && m.guardKind == GuardMerge && len(m.guardOptions) > 0 {
				opt := m.guardOptions[0]
				m.guardKind = 0
				m.guardOptions = nil
				m.guardLabel = ""
				return m, func() tea.Msg { return DataLossGuardResponseMsg{Response: opt.Response} }
			}
			// Cancel key (Escape) maps to the last option if it's Cancel
			if key.Matches(msg, m.keys.Cancel) && len(m.guardOptions) > 0 {
				opt := m.guardOptions[len(m.guardOptions)-1]
				m.guardKind = 0
				m.guardOptions = nil
				m.guardLabel = ""
				return m, func() tea.Msg { return DataLossGuardResponseMsg{Response: opt.Response} }
			}
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
			time.Sleep(errorDismissDelay)
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
		time.Sleep(confirmDelay)
		return confirmExpired{}
	}
}

func (m Model) View() string {
	// Error messages take precedence over normal status display.
	if m.errorMsg != "" {
		errContent := m.styles.Error.Render("⚠ " + m.errorMsg)
		return m.styles.Footer.Width(m.width).MaxHeight(1).Render(errContent)
	}

	// Default hint: the always-visible global shortcuts, rendered from the
	// bindings themselves so the footer can never drift from the keymap.
	k := func(b key.Binding) string { return m.styles.FooterKey.Render(b.Help().Key) }
	d := func(b key.Binding) string { return m.styles.FooterHint.Render(" " + b.Help().Desc) }
	left := k(m.keys.FocusExplorer) + d(m.keys.FocusExplorer) + "  " +
		k(m.keys.FocusEditor) + d(m.keys.FocusEditor) + "  " +
		k(m.keys.FocusChat) + d(m.keys.FocusChat) + "  " +
		k(m.keys.Help) + d(m.keys.Help)

	if m.dictating {
		left = m.styles.FooterKey.Render("^v") + m.styles.FooterHint.Render(" stop dictation")
	} else if m.guardKind == GuardDirty && len(m.guardOptions) > 0 {
		label := "Unsaved changes."
		if m.guardLabel != "" {
			label = m.guardLabel
		}
		left = m.styles.FooterKey.Render(label) +
			m.styles.FooterHint.Render(" [") +
			m.styles.FooterKey.Render("S") +
			m.styles.FooterHint.Render("]ave [") +
			m.styles.FooterKey.Render("D") +
			m.styles.FooterHint.Render("]iscard [") +
			m.styles.FooterKey.Render("Esc") +
			m.styles.FooterHint.Render("] Cancel")
	} else if m.guardKind == GuardMerge && len(m.guardOptions) > 0 {
		left = m.styles.FooterKey.Render("File changed on disk. Merge?") +
			m.styles.FooterHint.Render(" [") +
			m.styles.FooterKey.Render("Y") +
			m.styles.FooterHint.Render("]es [") +
			m.styles.FooterKey.Render("N") +
			m.styles.FooterHint.Render("]o")
	} else if m.pendingKey == "c" {
		left = m.styles.FooterKey.Render("Press ^C again to exit")
	} else if m.pendingKey == "d" {
		left = m.styles.FooterKey.Render("Press ^D again to exit")
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
