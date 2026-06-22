package textedit_test

import (
	"testing"

	"rune/pkg/editor/buffer"
	"rune/pkg/ui/components/textedit"
	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"
)

func newModel(content string) textedit.Model {
	m := textedit.New(keymap.Default(), styles.Default())
	return m.SetContent(content)
}

// TestReapplyCoalescedInserts is the regression test for the Undo×2→Redo
// data-loss bug. Rapid single-cursor typing coalesces into one journal event
// whose AppliedEdits are in ascending Start order. The old Reapply algorithm
// treated them as descending (multi-cursor) and produced invalid fwdEdits that
// buffer.ApplyEdits rejected, leaving the buffer silently unchanged.
func TestReapplyCoalescedInserts(t *testing.T) {
	m := newModel("")
	edits := []buffer.AppliedEdit{
		{Start: 0, Insert: "a"},
		{Start: 1, Insert: "b"},
	}
	m = m.Reapply(edits)
	if got := m.Content(); got != "ab" {
		t.Errorf("Reapply coalesced inserts: got %q, want %q", got, "ab")
	}
}

// TestReapplyMultiCursorInserts verifies Reapply with descending stored
// positions (multi-cursor batch — one ApplyEdits call with multiple cursors).
// Inserting 'a' at positions 0 and 5 of "hello world" via ApplyEdits produces
// applied = [{Start:6, Insert:"a"}, {Start:0, Insert:"a"}] (descending).
// Reapply must reproduce "ahelloa world" from "hello world".
func TestReapplyMultiCursorInserts(t *testing.T) {
	m := newModel("hello world")
	edits := []buffer.AppliedEdit{
		{Start: 6, Insert: "a"}, // descending order as stored in journal
		{Start: 0, Insert: "a"},
	}
	m = m.Reapply(edits)
	if got := m.Content(); got != "ahelloa world" {
		t.Errorf("Reapply multi-cursor inserts: got %q, want %q", got, "ahelloa world")
	}
}

// TestReapplySingleEdit guards the trivial single-edit case.
func TestReapplySingleEdit(t *testing.T) {
	m := newModel("hello")
	edits := []buffer.AppliedEdit{
		{Start: 5, Insert: " world"},
	}
	m = m.Reapply(edits)
	if got := m.Content(); got != "hello world" {
		t.Errorf("Reapply single edit: got %q, want %q", got, "hello world")
	}
}

// TestReapplyAfterUndoUndo drives the full Undo×2 → Redo cycle at the textedit
// layer to confirm Reapply correctly restores a coalesced multi-edit event.
func TestReapplyAfterUndoUndo(t *testing.T) {
	m := newModel("")

	// Event A edits: coalesced sequential inserts of 'a' then 'b'.
	eventA := []buffer.AppliedEdit{
		{Start: 0, Insert: "a"},
		{Start: 1, Insert: "b"},
	}
	// Event B edits: single insert of 'c'.
	eventB := []buffer.AppliedEdit{
		{Start: 2, Insert: "c"},
	}

	// Apply both events forward to reach "abc".
	m = m.Reapply(eventA)
	m = m.Reapply(eventB)
	if m.Content() != "abc" {
		t.Fatalf("setup: expected %q, got %q", "abc", m.Content())
	}

	// Undo B: "abc" → "ab".
	m = m.ApplyInverse(eventB)
	if m.Content() != "ab" {
		t.Fatalf("undo B: expected %q, got %q", "ab", m.Content())
	}

	// Undo A: "ab" → "".
	m = m.ApplyInverse(eventA)
	if m.Content() != "" {
		t.Fatalf("undo A: expected %q, got %q", "", m.Content())
	}

	// Redo A — the previously broken path.
	m = m.Reapply(eventA)
	if got := m.Content(); got != "ab" {
		t.Errorf("Undo×2 then Redo: got %q, want %q (DATA LOSS)", got, "ab")
	}
}
