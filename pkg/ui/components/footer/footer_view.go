package footer

import (
	"fmt"
	"strings"

	"charm.land/bubbles/v2/key"
	"charm.land/lipgloss/v2"
)

// displayMode is the single "what does the left side show" decision the
// footer makes each render — replacing the old 9-branch if/else priority
// ladder with one ordered table (F11). Declaration order IS priority order,
// highest first: modeError > modeDictating > modeGuard > modeChordPending >
// modeMergeHint > modeDiskChanged > modeDegraded > modeStatus > modeLinkHint
// > modeDefault.
type displayMode int

const (
	modeError displayMode = iota
	modeDictating
	modeGuard
	modeChordPending
	modeMergeHint
	modeDiskChanged
	modeDegraded
	modeStatus
	modeLinkHint
	modeDefault
)

// guardVisible reports whether the current guard state has a rendered
// descriptor AND live options — mirrors the original View()'s inline
// `desc, ok := guardDescriptorFor(m.guardKind); ok && len(m.guardOptions) > 0`
// condition exactly.
func (m Model) guardVisible() bool {
	_, ok := guardDescriptorFor(m.guardKind)
	return ok && len(m.guardOptions) > 0
}

// displayMode computes which single visual state the footer's left side
// shows for this View() call — the SINGLE priority table a transient
// status/link hint can never bypass to mask a guard (§1.4.4). Pure function
// of Model state; called once per View().
func (m Model) displayMode() displayMode {
	switch {
	case m.errorMsg != "":
		return modeError
	case m.dictating:
		return modeDictating
	case m.guardVisible():
		return modeGuard
	case m.pendingKey == "c" || m.pendingKey == "d":
		return modeChordPending
	case m.mergeActive:
		return modeMergeHint
	case m.diskChanged:
		return modeDiskChanged
	case m.degraded:
		return modeDegraded
	case m.statusMsg != "":
		return modeStatus
	case m.linkHint != "":
		return modeLinkHint
	default:
		return modeDefault
	}
}

// renderLeft renders the left side of the footer for the given mode.
// modeError is handled separately by View() (it replaces the whole footer
// content, not just the left side), so it never reaches here.
func (m Model) renderLeft(mode displayMode) string {
	switch mode {
	case modeDictating:
		return m.styles.FooterKey.Render("^v") + m.styles.FooterHint.Render(" stop dictation")

	case modeGuard:
		desc, _ := guardDescriptorFor(m.guardKind)
		label := desc.Label
		if m.guardKind == GuardDirty && m.guardLabel != "" {
			label = m.guardLabel
		}
		return renderGuardHint(m.styles, label, desc.Options)

	case modeChordPending:
		if m.pendingKey == "c" {
			return m.styles.FooterKey.Render("Press ^C again to exit")
		}
		return m.styles.FooterKey.Render("Press ^D again to exit")

	case modeMergeHint:
		// Persistent merge-resolver hint (§5). Not a guard (SetGuard is never
		// called for this) — it must never flip InGuard() true and steal keys
		// from the resolver (workspace_update_keys.go's guard pre-empt).
		return m.styles.FooterKey.Render("⚙ Merge") +
			m.styles.FooterHint.Render("  [") +
			m.styles.FooterKey.Render("O") +
			m.styles.FooterHint.Render("]urs [") +
			m.styles.FooterKey.Render("T") +
			m.styles.FooterHint.Render("]heirs  ·  ") +
			m.styles.FooterKey.Render("n") +
			m.styles.FooterHint.Render("/") +
			m.styles.FooterKey.Render("p") +
			m.styles.FooterHint.Render(" next·prev  ·  "+fmt.Sprintf("%d", m.mergeLeft)+" left")

	case modeDiskChanged:
		// Persistent "changed on disk" indicator (Fix C / BUG1) — unlike the
		// transient ShowStatusMsg text, this stays until the caller explicitly
		// clears it (a save, a tab switch to an unchanged file, or a guard
		// resolution that reconciles the divergence).
		return m.styles.FooterKey.Render("⚠") + m.styles.FooterHint.Render(" File changed on disk")

	case modeDegraded:
		// Persistent degraded-storage banner — capture-into-RAM must never
		// masquerade as durability; visible for the whole session (the
		// underlying Store.degraded bit is fixed at open time).
		return m.styles.FooterKey.Render("⚠") + m.styles.FooterHint.Render(" Storage degraded — history will not survive a crash")

	case modeStatus:
		// A transient status replaces the default hints but yields to
		// dictation/guard/chord above, so it never masks a prompt.
		return m.styles.FooterHint.Render(m.statusMsg)

	case modeLinkHint:
		// Lowest priority: the link-under-caret hint. Yields to everything above
		// (incl. the dirty guard, §1.4.4) so it can never mask a prompt.
		return m.styles.FooterHint.Render("→ "+m.linkHint+"  ") +
			m.styles.FooterKey.Render("⏎") + m.styles.FooterHint.Render(" open")

	default: // modeDefault
		// Default hint: the always-visible global shortcuts, rendered from the
		// bindings themselves so the footer can never drift from the keymap.
		k := func(b key.Binding) string { return m.styles.FooterKey.Render(b.Help().Key) }
		d := func(b key.Binding) string { return m.styles.FooterHint.Render(" " + b.Help().Desc) }
		return k(m.keys.FocusExplorer) + d(m.keys.FocusExplorer) + "  " +
			k(m.keys.FocusEditor) + d(m.keys.FocusEditor) + "  " +
			k(m.keys.FocusChat) + d(m.keys.FocusChat) + "  " +
			k(m.keys.Help) + d(m.keys.Help)
	}
}

func (m Model) View() string {
	mode := m.displayMode()

	// Error messages take precedence over normal status display — and unlike
	// every other mode, they replace the whole footer content rather than
	// just the left side (no Ln/Col/word-count/mic composition).
	if mode == modeError {
		errContent := m.styles.Error.Render("⚠ " + m.errorMsg)
		return m.styles.Footer.Width(m.width).MaxHeight(1).Render(errContent)
	}

	left := m.renderLeft(mode)

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
