package footer

import (
	"strings"
	"testing"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"
)

// newGuardMergeFooter returns a footer in GuardMerge state with the standard
// conflict guard options.
func newGuardMergeFooter() Model {
	m := New(keymap.Default(), styles.Default())
	m = m.SetSize(120, 1)
	opts := []GuardOption{
		{Key: 's', Response: DataLossSaveAnyway},
		{Key: 'd', Response: DataLossDiscard},
		{Key: 'm', Response: DataLossMerge},
		{Key: 0, Response: DataLossCancel},
	}
	return m.SetGuard(GuardMerge, opts)
}

// ─────────────────────────────────────────────────────────────────────────────
// View text
// ─────────────────────────────────────────────────────────────────────────────

// TestGuardMerge_ViewContainsAllKeyHints: the GuardMerge footer must render
// the full [S]ave anyway [D]iscard [M]erge [Esc] key hints so the user knows
// what choices are available.
func TestGuardMerge_ViewContainsAllKeyHints(t *testing.T) {
	m := newGuardMergeFooter()
	view := m.View()

	// Strip ANSI escape sequences for plain-text assertion.
	plain := stripAnsi(view)

	wants := []string{"S", "ave anyway", "D", "iscard", "M", "erge", "Esc"}
	for _, want := range wants {
		if !strings.Contains(plain, want) {
			t.Errorf("GuardMerge view missing %q\n  view: %q", want, plain)
		}
	}
}

// TestGuardMerge_ViewContainsChangedOnDisk: the GuardMerge footer must
// prominently mention the external-change context so the user understands why
// they are being prompted.
func TestGuardMerge_ViewContainsChangedOnDisk(t *testing.T) {
	m := newGuardMergeFooter()
	plain := stripAnsi(m.View())
	if !strings.Contains(plain, "changed on disk") {
		t.Errorf("GuardMerge view must mention 'changed on disk'; got: %q", plain)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// Enter → Cancel (R4)
// ─────────────────────────────────────────────────────────────────────────────

// TestGuardMerge_EnterMapsToCancel: pressing Enter while in GuardMerge must
// emit DataLossCancel, not DataLossSaveAnyway (R4). A stray Enter must never
// trigger a destructive save-anyway action.
func TestGuardMerge_EnterMapsToCancel(t *testing.T) {
	m := newGuardMergeFooter()
	_, cmd := m.Update(tea.KeyPressMsg{Code: tea.KeyEnter})
	if cmd == nil {
		t.Fatal("GuardMerge + Enter: expected a non-nil Cmd")
	}
	msg, ok := cmd().(DataLossGuardResponseMsg)
	if !ok {
		t.Fatalf("GuardMerge + Enter: expected DataLossGuardResponseMsg, got %T", cmd())
	}
	if msg.Response != DataLossCancel {
		t.Fatalf("GuardMerge + Enter: expected DataLossCancel (R4), got %v", msg.Response)
	}
}

// TestGuardDirty_EnterIsInert: Enter in GuardDirty must be ignored (returns
// nil Cmd). This guards the R4 behavior — Enter in GuardDirty must not
// accidentally forward to GuardMerge's Cancel logic; each guard kind is handled
// independently.
func TestGuardDirty_EnterIsInert(t *testing.T) {
	m := New(keymap.Default(), styles.Default())
	m = m.SetSize(120, 1)
	opts := []GuardOption{
		{Key: 's', Response: DataLossSave},
		{Key: 'd', Response: DataLossDiscard},
		{Key: 0, Response: DataLossCancel},
	}
	m = m.SetGuard(GuardDirty, opts)

	_, cmd := m.Update(tea.KeyPressMsg{Code: tea.KeyEnter})
	if cmd != nil {
		// Guard must still be active (no resolution happened)
		t.Fatal("GuardDirty + Enter: expected nil Cmd (Enter is intentionally inert in GuardDirty)")
	}
	if !m.InGuard() {
		t.Fatal("GuardDirty + Enter: guard must remain active when Enter is pressed")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// Key routing in GuardMerge
// ─────────────────────────────────────────────────────────────────────────────

// TestGuardMerge_SKeyEmitsSaveAnyway: pressing 's' in GuardMerge must emit
// DataLossSaveAnyway.
func TestGuardMerge_SKeyEmitsSaveAnyway(t *testing.T) {
	m := newGuardMergeFooter()
	_, cmd := m.Update(tea.KeyPressMsg{Code: 's', Text: "s"})
	if cmd == nil {
		t.Fatal("GuardMerge + 's': expected Cmd")
	}
	msg, ok := cmd().(DataLossGuardResponseMsg)
	if !ok || msg.Response != DataLossSaveAnyway {
		t.Fatalf("GuardMerge + 's': expected DataLossSaveAnyway, got %v (ok=%v)", msg.Response, ok)
	}
}

// TestGuardMerge_DKeyEmitsDiscard: pressing 'd' in GuardMerge must emit
// DataLossDiscard.
func TestGuardMerge_DKeyEmitsDiscard(t *testing.T) {
	m := newGuardMergeFooter()
	_, cmd := m.Update(tea.KeyPressMsg{Code: 'd', Text: "d"})
	if cmd == nil {
		t.Fatal("GuardMerge + 'd': expected Cmd")
	}
	msg, ok := cmd().(DataLossGuardResponseMsg)
	if !ok || msg.Response != DataLossDiscard {
		t.Fatalf("GuardMerge + 'd': expected DataLossDiscard, got %v (ok=%v)", msg.Response, ok)
	}
}

// TestGuardMerge_MKeyEmitsMerge: pressing 'm' in GuardMerge must emit
// DataLossMerge.
func TestGuardMerge_MKeyEmitsMerge(t *testing.T) {
	m := newGuardMergeFooter()
	_, cmd := m.Update(tea.KeyPressMsg{Code: 'm', Text: "m"})
	if cmd == nil {
		t.Fatal("GuardMerge + 'm': expected Cmd")
	}
	msg, ok := cmd().(DataLossGuardResponseMsg)
	if !ok || msg.Response != DataLossMerge {
		t.Fatalf("GuardMerge + 'm': expected DataLossMerge, got %v (ok=%v)", msg.Response, ok)
	}
}

// TestGuardMerge_EscEmitsCancel: pressing Escape in GuardMerge must emit
// DataLossCancel.
func TestGuardMerge_EscEmitsCancel(t *testing.T) {
	m := newGuardMergeFooter()
	_, cmd := m.Update(tea.KeyPressMsg{Code: tea.KeyEscape})
	if cmd == nil {
		t.Fatal("GuardMerge + Esc: expected Cmd")
	}
	msg, ok := cmd().(DataLossGuardResponseMsg)
	if !ok || msg.Response != DataLossCancel {
		t.Fatalf("GuardMerge + Esc: expected DataLossCancel, got %v (ok=%v)", msg.Response, ok)
	}
}

// TestGuardMerge_GuardIsCleared: after any valid response, the guard must be
// cleared so a follow-up key does not re-trigger a phantom action.
func TestGuardMerge_GuardIsCleared(t *testing.T) {
	m := newGuardMergeFooter()
	m, _ = m.Update(tea.KeyPressMsg{Code: 's', Text: "s"})
	if m.InGuard() {
		t.Fatal("guard must be cleared after a response key is pressed")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// DataLossGuardResponse constants uniqueness
// ─────────────────────────────────────────────────────────────────────────────

// TestDataLossGuardResponse_ValuesUnique: all DataLossGuardResponse constants
// must be distinct so switch statements on response values work correctly.
func TestDataLossGuardResponse_ValuesUnique(t *testing.T) {
	seen := map[DataLossGuardResponse]string{}
	pairs := []struct {
		name string
		val  DataLossGuardResponse
	}{
		{"DataLossSave", DataLossSave},
		{"DataLossDiscard", DataLossDiscard},
		{"DataLossCancel", DataLossCancel},
		{"DataLossMergeAccept", DataLossMergeAccept},
		{"DataLossMergeReject", DataLossMergeReject},
		{"DataLossSaveAnyway", DataLossSaveAnyway},
		{"DataLossMerge", DataLossMerge},
	}
	for _, p := range pairs {
		if prev, exists := seen[p.val]; exists {
			t.Errorf("duplicate DataLossGuardResponse value %d: %s and %s", p.val, prev, p.name)
		}
		seen[p.val] = p.name
	}
}

// stripAnsi removes ANSI/CSI escape sequences from a string for plain-text
// comparison in view assertions. This is a minimal implementation sufficient
// for footer view tests.
func stripAnsi(s string) string {
	var b strings.Builder
	i := 0
	for i < len(s) {
		if s[i] == '\x1b' && i+1 < len(s) && s[i+1] == '[' {
			// Skip until final byte (0x40–0x7E)
			i += 2
			for i < len(s) && (s[i] < 0x40 || s[i] > 0x7E) {
				i++
			}
			i++ // skip the final byte
			continue
		}
		b.WriteByte(s[i])
		i++
	}
	return b.String()
}
