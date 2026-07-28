package footer

import (
	"strings"
	"testing"

	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"
)

func newFooter() Model {
	return New(keymap.Default(), styles.Default()).SetSize(80, 1)
}

func TestShowStatusMsgRenders(t *testing.T) {
	m := newFooter()
	m, cmd := m.Update(ShowStatusMsg{Text: "→ other.md"})
	if cmd == nil {
		t.Fatal("ShowStatusMsg should return a dismiss-timer cmd")
	}
	if !strings.Contains(m.View(), "other.md") {
		t.Errorf("status not rendered; View=%q", m.View())
	}
}

// A transient status must never mask a dirty guard prompt (§1.4.4).
func TestStatusYieldsToDirtyGuard(t *testing.T) {
	m := newFooter()
	m, _ = m.Update(ShowStatusMsg{Text: "→ other.md"})
	m = m.SetGuard(GuardDirty, []GuardOption{{Key: 's', Response: DataLossSave}})
	view := m.View()
	if strings.Contains(view, "other.md") {
		t.Errorf("status masked the dirty guard; View=%q", view)
	}
	if !strings.Contains(view, "Unsaved") {
		t.Errorf("dirty guard not shown; View=%q", view)
	}
}

func TestLinkHintRenders(t *testing.T) {
	m := newFooter()
	m, _ = m.Update(UpdateCursorMsg{LinkTarget: "pages/foo.md"})
	v := m.View()
	if !strings.Contains(v, "pages/foo.md") || !strings.Contains(v, "open") {
		t.Errorf("link hint not rendered; View=%q", v)
	}
	// Clearing the target (caret left the link) removes the hint.
	m, _ = m.Update(UpdateCursorMsg{LinkTarget: ""})
	if strings.Contains(m.View(), "pages/foo.md") {
		t.Errorf("link hint should clear when target is empty; View=%q", m.View())
	}
}

// The link hint is lowest-precedence and must never mask the dirty guard (§1.4.4).
func TestLinkHintYieldsToDirtyGuard(t *testing.T) {
	m := newFooter()
	m, _ = m.Update(UpdateCursorMsg{LinkTarget: "pages/foo.md"})
	m = m.SetGuard(GuardDirty, []GuardOption{{Key: 's', Response: DataLossSave}})
	v := m.View()
	if strings.Contains(v, "pages/foo.md") {
		t.Errorf("link hint masked the dirty guard; View=%q", v)
	}
	if !strings.Contains(v, "Unsaved") {
		t.Errorf("dirty guard not shown; View=%q", v)
	}
}

func TestStatusDismissed(t *testing.T) {
	m := newFooter()
	m, _ = m.Update(ShowStatusMsg{Text: "→ other.md"})
	m, _ = m.Update(statusDismissedMsg{id: m.statusExpireID})
	if strings.Contains(m.View(), "other.md") {
		t.Error("status should clear after dismiss")
	}
}
