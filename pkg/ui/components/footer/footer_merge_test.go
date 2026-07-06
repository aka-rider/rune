package footer

import (
	"strings"
	"testing"

	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"
)

// TestSetMergeMode_RendersHint: SetMergeMode(true, n) must render the
// persistent "[O]urs [T]heirs ... N left" hint (§5).
func TestSetMergeMode_RendersHint(t *testing.T) {
	m := New(keymap.Default(), styles.Default())
	m = m.SetSize(120, 1)
	m = m.SetMergeMode(true, 3)

	plain := stripAnsi(m.View())
	for _, want := range []string{"Merge", "O", "urs", "T", "heirs", "3", "left"} {
		if !strings.Contains(plain, want) {
			t.Errorf("merge hint missing %q\n  view: %q", want, plain)
		}
	}
}

// TestSetMergeMode_ClearsWhenInactive: SetMergeMode(false, 0) must not show
// the merge hint (falls through to the default global-shortcut hints).
func TestSetMergeMode_ClearsWhenInactive(t *testing.T) {
	m := New(keymap.Default(), styles.Default())
	m = m.SetSize(120, 1)
	m = m.SetMergeMode(true, 1)
	m = m.SetMergeMode(false, 0)

	plain := stripAnsi(m.View())
	if strings.Contains(plain, "⚙ Merge") {
		t.Errorf("merge hint must not render once inactive: %q", plain)
	}
}

// TestSetMergeMode_YieldsToGuard: an active data-loss guard (e.g. GuardDirty,
// raised over a merge-inactive footer state, or GuardMerge itself) must still
// win over the merge hint — the hint sits BELOW the guards in the priority
// chain (§5) so a guard is never masked.
func TestSetMergeMode_YieldsToGuard(t *testing.T) {
	m := New(keymap.Default(), styles.Default())
	m = m.SetSize(120, 1)
	m = m.SetMergeMode(true, 2)

	opts := []GuardOption{
		{Key: 's', Response: DataLossSave},
		{Key: 'd', Response: DataLossDiscard},
		{Key: 0, Response: DataLossCancel},
	}
	m = m.SetGuard(GuardDirty, opts)

	plain := stripAnsi(m.View())
	if strings.Contains(plain, "⚙ Merge") {
		t.Errorf("guard must win over the merge hint: %q", plain)
	}
	if !strings.Contains(plain, "Unsaved changes") {
		t.Errorf("expected the dirty guard to render instead: %q", plain)
	}
}

// TestSetMergeMode_NeverSetsGuard: SetMergeMode must not flip InGuard() true
// — doing so would steal keys from the merge resolver's own key routing
// (workspace_update_keys.go's guard pre-empt runs before the paneCenter merge
// intercept).
func TestSetMergeMode_NeverSetsGuard(t *testing.T) {
	m := New(keymap.Default(), styles.Default())
	m = m.SetMergeMode(true, 5)
	if m.InGuard() {
		t.Fatal("SetMergeMode must never set InGuard() true")
	}
}
