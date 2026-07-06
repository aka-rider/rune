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
	GuardTrash
	// GuardDeleted gates recovery when the current document's file has gone
	// missing on disk (deleted, or its parent dir removed) — mirrors GuardMerge
	// but with no "theirs" to diff against (§1.4.7).
	GuardDeleted
	// GuardRaced gates a Materialize swap-race outcome (F5): our write
	// committed for real, but a concurrent writer's bytes were displaced and
	// captured (never lost — I1). A DISTINCT guard from GuardMerge: there is
	// no live disk divergence to re-probe (a fresh read would just find OUR
	// already-committed bytes), only a choice between keeping our write or
	// restoring the captured displaced bytes on top of it.
	GuardRaced
	// GuardDegraded confirms an explicit write while the store is degraded
	// (docstate.Store.Degraded(): capture-into-RAM masquerading as
	// durability — the recovery journal will NOT survive a crash for the
	// rest of this session). Raised before an interactive save proceeds.
	GuardDegraded
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
	DataLossTrash
	// DataLossSaveAnyway is the [S]ave anyway response for the conflict guard
	// (GuardMerge): the user acknowledges the external change and overwrites.
	DataLossSaveAnyway
	// DataLossMerge is the [M]erge response for the conflict guard (GuardMerge):
	// the user requests the interactive merge resolver (Phase 2).
	DataLossMerge
	// DataLossKeepMine is the [K]eep-mine response for the swap-race guard
	// (GuardRaced, F5): our already-committed write stands; the displaced
	// bytes remain recoverable history but are not restored to disk.
	DataLossKeepMine
	// DataLossRestoreTheirs is the [R]estore-theirs response for the
	// swap-race guard (GuardRaced, F5): the captured displaced bytes are
	// written back to disk, on top of our already-committed write.
	DataLossRestoreTheirs
	// DataLossConfirmDegraded is the [Y]es response for the degraded-store
	// guard (GuardDegraded): the user acknowledges the save proceeds without
	// a durable recovery journal for the rest of this session.
	DataLossConfirmDegraded
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

// resolveGuard is the chokepoint all three guard-resolution paths in Update
// funnel through — a typed key matching one of m.guardOptions, Enter mapping
// to the last option (GuardMerge/GuardDeleted only), and Cancel mapping to
// the last option: clear the guard state and emit opt's response. Each
// caller still decides WHICH opt to resolve with; this only centralizes what
// happens once one is chosen.
func (m Model) resolveGuard(opt GuardOption) (Model, tea.Cmd) {
	m.guardKind = 0
	m.guardOptions = nil
	m.guardLabel = ""
	return m, func() tea.Msg { return DataLossGuardResponseMsg{Response: opt.Response} }
}

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

// guardOptionHint is one keyed option in a guard's rendered hint, e.g.
// "[S]ave": Key is FooterKey-styled, Suffix is FooterHint-styled and
// immediately follows the closing "]" (a word-continuation like "ave" needs
// no leading space; a standalone word like " Cancel" supplies its own).
type guardOptionHint struct {
	Key    string
	Suffix string
}

// guardDescriptor is the render recipe for one GuardKind: a label followed
// by its keyed option hints.
type guardDescriptor struct {
	Label   string
	Options []guardOptionHint
}

// guardDescriptorFor is the single source View's guard-render arm reads
// from — before this chokepoint, each of the 6 guard kinds independently
// built its own "label + [Key]suffix [Key]suffix..." construction inline.
// ok is false for a GuardKind with no descriptor (there is none today; every
// declared GuardKind has one).
func guardDescriptorFor(kind GuardKind) (guardDescriptor, bool) {
	switch kind {
	case GuardDirty:
		// Label defaults to "Unsaved changes." — View overrides it with
		// m.guardLabel when set (e.g. the eviction victim's filename).
		return guardDescriptor{
			Label:   "Unsaved changes.",
			Options: []guardOptionHint{{"S", "ave"}, {"D", "iscard"}, {"Esc", " Cancel"}},
		}, true
	case GuardMerge:
		// R4: [S]ave anyway [D]iscard [M]erge [Esc] — Enter is neutralized to Cancel.
		return guardDescriptor{
			Label:   "File changed on disk.",
			Options: []guardOptionHint{{"S", "ave anyway"}, {"D", "iscard"}, {"M", "erge"}, {"Esc", ""}},
		}, true
	case GuardDeleted:
		// Mirrors GuardMerge's rendering, but there is no "theirs" to diff
		// against — only [S]ave (recreate) / [D]iscard (purge) / Esc.
		return guardDescriptor{
			Label:   "File deleted on disk.",
			Options: []guardOptionHint{{"S", "ave"}, {"D", "iscard"}, {"Esc", ""}},
		}, true
	case GuardRaced:
		return guardDescriptor{
			Label:   "Save raced with a concurrent write.",
			Options: []guardOptionHint{{"K", "eep mine"}, {"R", "estore theirs"}, {"Esc", ""}},
		}, true
	case GuardDegraded:
		return guardDescriptor{
			Label:   "Storage degraded — history will not survive a crash.",
			Options: []guardOptionHint{{"Y", "es, save anyway"}, {"Esc", " Cancel"}},
		}, true
	case GuardTrash:
		return guardDescriptor{
			Label:   "Trash file?",
			Options: []guardOptionHint{{"Y", "es"}, {"Esc", " Cancel"}},
		}, true
	}
	return guardDescriptor{}, false
}

// renderGuardHint renders one guard descriptor's label followed by its
// "[Key]suffix" options, each separated by a space — byte-identical to the
// inline construction it replaces.
func renderGuardHint(st styles.Styles, label string, opts []guardOptionHint) string {
	s := st.FooterKey.Render(label)
	for _, o := range opts {
		s += st.FooterHint.Render(" [") + st.FooterKey.Render(o.Key) + st.FooterHint.Render("]"+o.Suffix)
	}
	return s
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
	} else if desc, ok := guardDescriptorFor(m.guardKind); ok && len(m.guardOptions) > 0 {
		label := desc.Label
		if m.guardKind == GuardDirty && m.guardLabel != "" {
			label = m.guardLabel
		}
		left = renderGuardHint(m.styles, label, desc.Options)
	} else if m.pendingKey == "c" {
		left = m.styles.FooterKey.Render("Press ^C again to exit")
	} else if m.pendingKey == "d" {
		left = m.styles.FooterKey.Render("Press ^D again to exit")
	} else if m.mergeActive {
		// Persistent merge-resolver hint (§5). Not a guard (SetGuard is never
		// called for this) — it must never flip InGuard() true and steal keys
		// from the resolver (workspace_update_keys.go's guard pre-empt).
		left = m.styles.FooterKey.Render("⚙ Merge") +
			m.styles.FooterHint.Render("  [") +
			m.styles.FooterKey.Render("O") +
			m.styles.FooterHint.Render("]urs [") +
			m.styles.FooterKey.Render("T") +
			m.styles.FooterHint.Render("]heirs  ·  ") +
			m.styles.FooterKey.Render("n") +
			m.styles.FooterHint.Render("/") +
			m.styles.FooterKey.Render("p") +
			m.styles.FooterHint.Render(" next·prev  ·  "+fmt.Sprintf("%d", m.mergeLeft)+" left")
	} else if m.diskChanged {
		// Persistent "changed on disk" indicator (Fix C / BUG1) — unlike the
		// transient ShowStatusMsg text, this stays until the caller explicitly
		// clears it (a save, a tab switch to an unchanged file, or a guard
		// resolution that reconciles the divergence).
		left = m.styles.FooterKey.Render("⚠") + m.styles.FooterHint.Render(" File changed on disk")
	} else if m.degraded {
		// Persistent degraded-storage banner — capture-into-RAM must never
		// masquerade as durability; visible for the whole session (the
		// underlying Store.degraded bit is fixed at open time).
		left = m.styles.FooterKey.Render("⚠") + m.styles.FooterHint.Render(" Storage degraded — history will not survive a crash")
	} else if m.statusMsg != "" {
		// A transient status replaces the default hints but yields to
		// dictation/guard/chord above, so it never masks a prompt.
		left = m.styles.FooterHint.Render(m.statusMsg)
	} else if m.linkHint != "" {
		// Lowest priority: the link-under-caret hint. Yields to everything above
		// (incl. the dirty guard, §1.4.4) so it can never mask a prompt.
		left = m.styles.FooterHint.Render("→ "+m.linkHint+"  ") +
			m.styles.FooterKey.Render("⏎") + m.styles.FooterHint.Render(" open")
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
