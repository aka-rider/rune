package footer

import (
	"time"

	"charm.land/bubbles/v2/key"
	tea "charm.land/bubbletea/v2"

	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"
)

// ConfirmQuitMsg is emitted when a chord exit sequence completes (e.g., ^C^C).
type ConfirmQuitMsg struct{}

// confirmExpired is an internal message to reset chord state after timeout.
type confirmExpired struct{}

// ShowErrorMsg tells the footer to display a transient error message.
type ShowErrorMsg struct{ Text string }

// errorDismissedMsg is an internal timer message to clear the error after timeout.
type errorDismissedMsg struct{ id int }

// ShowStatusMsg tells the footer to display a transient, neutral status message
// (e.g. a link-follow confirmation). Auto-dismisses like ShowErrorMsg, but
// yields to a guard/chord prompt so it can never mask one (§1.4.4).
type ShowStatusMsg struct{ Text string }

// statusDismissedMsg is an internal timer message to clear the status after timeout.
type statusDismissedMsg struct{ id int }

type UpdateCursorMsg struct {
	Line       int
	Col        int
	WordCount  int
	LinkTarget string // raw target of the link under the caret ("" if none)
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
	mergeActive      bool
	mergeLeft        int
	diskChanged      bool
	degraded         bool
	errorMsg         string
	errorExpireID    int
	statusMsg        string
	statusExpireID   int
	linkHint         string
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

// SetMergeMode mirrors the workspace's mergemode.State onto the footer so the
// persistent "[O]urs [T]heirs ... N left" hint (§5) tracks the resolver
// without the footer importing mergemode itself (mirrors SetDictationAllowed).
func (m Model) SetMergeMode(active bool, conflictsLeft int) Model {
	m.mergeActive = active
	m.mergeLeft = conflictsLeft
	return m
}

// SetDiskChanged mirrors the workspace's diskChangedHint onto the footer
// (Fix C / BUG1) so the "changed on disk" indicator is a PERSISTENT hint —
// visible until explicitly cleared — rather than only the transient
// ShowStatusMsg text that auto-dismisses after a few seconds. Yields to every
// guard and the merge hint in the priority chain (never a guard itself).
func (m Model) SetDiskChanged(changed bool) Model { m.diskChanged = changed; return m }

// SetDegraded mirrors docstate.Store.Degraded() onto the footer: a
// PERSISTENT banner (never a transient ShowErrorMsg that auto-dismisses)
// warning that recovery history is capture-into-RAM only for the rest of
// this session — never a promise of durability. Set once when the store
// becomes ready and never legitimately un-set within a session (a Store's
// degraded bit is fixed at open time).
func (m Model) SetDegraded(degraded bool) Model { m.degraded = degraded; return m }

// Degraded reports the persistent degraded-store banner state (SetDegraded).
// Production code never reads it back; the fuzz harness's invariant monitors
// do (workspace_fuzz.go, built with -tags fuzzing — a caller a plain build's
// dead-code check does not see).
func (m Model) Degraded() bool { return m.degraded }

func (m Model) Init() tea.Cmd { return nil }

func (m Model) Update(msg tea.Msg) (Model, tea.Cmd) {
	switch msg := msg.(type) {
	case tea.KeyPressMsg:
		// Guard mode consumes all keypresses until resolved.
		if len(m.guardOptions) > 0 {
			for _, opt := range m.guardOptions {
				if opt.Key != 0 && msg.Text == string(opt.Key) && msg.Mod == 0 {
					return m.resolveGuard(opt)
				}
			}
			// R4: Enter on GuardMerge/GuardDeleted maps to Cancel (the last
			// option), not the first. A stray Enter must never pick a
			// destructive [S]ave (anyway). GuardDirty does NOT resolve on Enter
			// (users expect Enter inert while the dirty prompt is active), so
			// this branch excludes it.
			if msg.Code == tea.KeyEnter && (m.guardKind == GuardMerge || m.guardKind == GuardDeleted) && len(m.guardOptions) > 0 {
				return m.resolveGuard(m.guardOptions[len(m.guardOptions)-1]) // last = Cancel
			}
			// Cancel key (Escape) maps to the last option if it's Cancel
			if key.Matches(msg, m.keys.Cancel) && len(m.guardOptions) > 0 {
				return m.resolveGuard(m.guardOptions[len(m.guardOptions)-1])
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
		m.linkHint = msg.LinkTarget

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

	case ShowStatusMsg:
		m.statusMsg = msg.Text
		m.statusExpireID++
		id := m.statusExpireID
		return m, func() tea.Msg {
			time.Sleep(errorDismissDelay)
			return statusDismissedMsg{id: id}
		}

	case statusDismissedMsg:
		if msg.id == m.statusExpireID {
			m.statusMsg = ""
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
