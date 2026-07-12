package footer

import (
	"strings"
	"testing"

	tea "charm.land/bubbletea/v2"

	"rune/internal/editortest"
	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"
)

// newGuardDeletedFooter returns a footer in GuardDeleted state with the
// standard file-deleted-on-disk guard options.
func newGuardDeletedFooter() Model {
	m := New(keymap.Default(), styles.Default())
	m = m.SetSize(120, 1)
	opts := []GuardOption{
		{Key: 's', Response: DataLossSaveAnyway},
		{Key: 'd', Response: DataLossDiscard},
		{Key: 0, Response: DataLossCancel},
	}
	return m.SetGuard(GuardDeleted, opts)
}

// ─────────────────────────────────────────────────────────────────────────────
// View text
// ─────────────────────────────────────────────────────────────────────────────

// TestGuardDeleted_ViewExactText: the GuardDeleted footer must render the exact
// "File deleted on disk. [S]ave [D]iscard [Esc]" prompt (plan §ACTIVE(2)).
func TestGuardDeleted_ViewExactText(t *testing.T) {
	m := newGuardDeletedFooter()
	plain := editortest.StripANSI(m.View())
	const want = "File deleted on disk. [S]ave [D]iscard [Esc]"
	if !strings.Contains(plain, want) {
		t.Errorf("GuardDeleted view = %q, want it to contain %q", plain, want)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// Enter → Cancel (R4, mirrors GuardMerge)
// ─────────────────────────────────────────────────────────────────────────────

// TestGuardDeleted_EnterMapsToCancel: pressing Enter while in GuardDeleted must
// emit DataLossCancel, not DataLossSaveAnyway — a stray Enter must never
// trigger the destructive recreate-save.
func TestGuardDeleted_EnterMapsToCancel(t *testing.T) {
	m := newGuardDeletedFooter()
	_, cmd := m.Update(tea.KeyPressMsg{Code: tea.KeyEnter})
	if cmd == nil {
		t.Fatal("GuardDeleted + Enter: expected a non-nil Cmd")
	}
	msg, ok := cmd().(DataLossGuardResponseMsg)
	if !ok {
		t.Fatalf("GuardDeleted + Enter: expected DataLossGuardResponseMsg, got %T", cmd())
	}
	if msg.Response != DataLossCancel {
		t.Fatalf("GuardDeleted + Enter: expected DataLossCancel, got %v", msg.Response)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// Key routing in GuardDeleted
// ─────────────────────────────────────────────────────────────────────────────

func TestGuardDeleted_SKeyEmitsSaveAnyway(t *testing.T) {
	m := newGuardDeletedFooter()
	_, cmd := m.Update(tea.KeyPressMsg{Code: 's', Text: "s"})
	if cmd == nil {
		t.Fatal("GuardDeleted + 's': expected Cmd")
	}
	msg, ok := cmd().(DataLossGuardResponseMsg)
	if !ok || msg.Response != DataLossSaveAnyway {
		t.Fatalf("GuardDeleted + 's': expected DataLossSaveAnyway, got %v (ok=%v)", msg.Response, ok)
	}
}

func TestGuardDeleted_DKeyEmitsDiscard(t *testing.T) {
	m := newGuardDeletedFooter()
	_, cmd := m.Update(tea.KeyPressMsg{Code: 'd', Text: "d"})
	if cmd == nil {
		t.Fatal("GuardDeleted + 'd': expected Cmd")
	}
	msg, ok := cmd().(DataLossGuardResponseMsg)
	if !ok || msg.Response != DataLossDiscard {
		t.Fatalf("GuardDeleted + 'd': expected DataLossDiscard, got %v (ok=%v)", msg.Response, ok)
	}
}

func TestGuardDeleted_EscEmitsCancel(t *testing.T) {
	m := newGuardDeletedFooter()
	_, cmd := m.Update(tea.KeyPressMsg{Code: tea.KeyEscape})
	if cmd == nil {
		t.Fatal("GuardDeleted + Esc: expected Cmd")
	}
	msg, ok := cmd().(DataLossGuardResponseMsg)
	if !ok || msg.Response != DataLossCancel {
		t.Fatalf("GuardDeleted + Esc: expected DataLossCancel, got %v (ok=%v)", msg.Response, ok)
	}
}

// TestGuardDeleted_GuardIsCleared: after any valid response, the guard must be
// cleared so a follow-up key does not re-trigger a phantom action.
func TestGuardDeleted_GuardIsCleared(t *testing.T) {
	m := newGuardDeletedFooter()
	m, _ = m.Update(tea.KeyPressMsg{Code: 's', Text: "s"})
	if m.InGuard() {
		t.Fatal("guard must be cleared after a response key is pressed")
	}
}
