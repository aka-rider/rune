package footer

import (
	"strings"
	"testing"

	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"
)

// TestSetDiskChanged_RendersPersistentHint: SetDiskChanged(true) must render a
// persistent "changed on disk" hint (Fix C / BUG1) — distinct from the
// transient ShowStatusMsg text, which auto-dismisses after a few seconds.
func TestSetDiskChanged_RendersPersistentHint(t *testing.T) {
	m := New(keymap.Default(), styles.Default())
	m = m.SetSize(120, 1)
	m = m.SetDiskChanged(true)

	plain := stripAnsi(m.View())
	if !strings.Contains(plain, "File changed on disk") {
		t.Errorf("disk-changed hint missing from footer view: %q", plain)
	}
}

// TestSetDiskChanged_ClearsWhenFalse: SetDiskChanged(false) must not show the
// hint (falls through to the default global-shortcut hints).
func TestSetDiskChanged_ClearsWhenFalse(t *testing.T) {
	m := New(keymap.Default(), styles.Default())
	m = m.SetSize(120, 1)
	m = m.SetDiskChanged(true)
	m = m.SetDiskChanged(false)

	plain := stripAnsi(m.View())
	if strings.Contains(plain, "File changed on disk") {
		t.Errorf("disk-changed hint must not render once cleared: %q", plain)
	}
}

// TestSetDiskChanged_YieldsToGuard: an active data-loss guard must still win
// over the persistent disk-changed hint — the hint sits below the guards in
// the priority chain, so a guard is never masked (§1.4.4).
func TestSetDiskChanged_YieldsToGuard(t *testing.T) {
	m := New(keymap.Default(), styles.Default())
	m = m.SetSize(120, 1)
	m = m.SetDiskChanged(true)

	opts := []GuardOption{
		{Key: 's', Response: DataLossSave},
		{Key: 'd', Response: DataLossDiscard},
		{Key: 0, Response: DataLossCancel},
	}
	m = m.SetGuard(GuardDirty, opts)

	plain := stripAnsi(m.View())
	if strings.Contains(plain, "File changed on disk") {
		t.Errorf("guard must win over the disk-changed hint: %q", plain)
	}
	if !strings.Contains(plain, "Unsaved changes") {
		t.Errorf("expected the dirty guard to render instead: %q", plain)
	}
}

// TestSetDiskChanged_YieldsToMergeHint: the persistent merge hint takes
// priority over the disk-changed hint (the two should not normally coincide —
// diskChangedHint is cleared at [M]/[D] — but the render order must still be
// deterministic if they ever do).
func TestSetDiskChanged_YieldsToMergeHint(t *testing.T) {
	m := New(keymap.Default(), styles.Default())
	m = m.SetSize(120, 1)
	m = m.SetDiskChanged(true)
	m = m.SetMergeMode(true, 1)

	plain := stripAnsi(m.View())
	if !strings.Contains(plain, "⚙ Merge") {
		t.Errorf("merge hint must win over the disk-changed hint: %q", plain)
	}
}

// TestSetDiskChanged_NeverSetsGuard: SetDiskChanged must not flip InGuard()
// true — it is a passive indicator, never a modal prompt.
func TestSetDiskChanged_NeverSetsGuard(t *testing.T) {
	m := New(keymap.Default(), styles.Default())
	m = m.SetDiskChanged(true)
	if m.InGuard() {
		t.Fatal("SetDiskChanged must never set InGuard() true")
	}
}
