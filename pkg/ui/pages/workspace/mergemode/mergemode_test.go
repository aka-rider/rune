package mergemode

import (
	"strings"
	"testing"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/merge"
	"rune/pkg/terminal"
	"rune/pkg/ui/components/markdownedit"
	"rune/pkg/ui/components/textedit"
	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"
)

// newEditor returns a fresh markdownedit Model sized for merge tests.
func newEditor(t *testing.T) markdownedit.Model {
	t.Helper()
	m := markdownedit.New(keymap.Default(), styles.Default(), terminal.TermCaps{})
	m = m.SetRect(textedit.Rect{W: 80, H: 20})
	return m
}

// newState returns a fresh mergemode State sized to match newEditor.
func newState(t *testing.T) State {
	t.Helper()
	st := New(keymap.Default(), styles.Default())
	st = SetSize(st, 80, 20)
	return st
}

// hunks builds a minimal hunk slice for tests.
func hunks(kind merge.HunkKind, ours, theirs string) []merge.Hunk {
	if kind == merge.HunkClean {
		return []merge.Hunk{{Kind: merge.HunkClean, AutoBytes: []byte(ours)}}
	}
	return []merge.Hunk{{Kind: merge.HunkConflict, OursBytes: []byte(ours), TheirsBytes: []byte(theirs)}}
}

func press(code rune) tea.KeyPressMsg {
	return tea.KeyPressMsg{Code: code, Text: string(code)}
}

// ─────────────────────────────────────────────────────────────────────────────
// Enter: clean merge (no conflicts)
// ─────────────────────────────────────────────────────────────────────────────

func TestEnter_CleanHunk(t *testing.T) {
	st, ed := newState(t), newEditor(t)
	st, ed, _, _ = Enter(hunks(merge.HunkClean, "clean content\n", ""), st, ed)

	if IsActive(st) {
		t.Fatal("clean-only hunks: IsActive should be false (nothing to resolve)")
	}
	if HasUnresolvedConflicts(st) {
		t.Fatal("clean-only hunks: HasUnresolvedConflicts should be false")
	}
	if got := ed.Content(); got != "clean content\n" {
		t.Fatalf("clean-only hunks: buffer=%q, want %q", got, "clean content\n")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// Enter: conflict hunks — buffer holds BOTH sides as markers (§3, unlike the
// old markdownedit-only-ours behavior)
// ─────────────────────────────────────────────────────────────────────────────

func TestEnter_ConflictHunk_BufferHoldsBothSides(t *testing.T) {
	const oursContent = "ours version\n"
	const theirsContent = "theirs version\n"

	st, ed := newState(t), newEditor(t)
	st, ed, _, _ = Enter(hunks(merge.HunkConflict, oursContent, theirsContent), st, ed)

	if !IsActive(st) {
		t.Fatal("conflict hunk: IsActive should be true")
	}
	if !HasUnresolvedConflicts(st) {
		t.Fatal("conflict hunk: HasUnresolvedConflicts should be true")
	}

	got := ed.Content()
	for _, want := range []string{"<<<<<<< ours", oursContent, "=======", theirsContent, ">>>>>>> theirs"} {
		if !strings.Contains(got, want) {
			t.Fatalf("Enter: buffer=%q missing %q — the live buffer must hold BOTH sides as markers (§3)", got, want)
		}
	}
}

func TestEnter_MultipleConflicts_IntervalsInOrder(t *testing.T) {
	input := []merge.Hunk{
		{Kind: merge.HunkClean, AutoBytes: []byte("header\n")},
		{Kind: merge.HunkConflict, OursBytes: []byte("ours-A\n"), TheirsBytes: []byte("theirs-A\n")},
		{Kind: merge.HunkClean, AutoBytes: []byte("middle\n")},
		{Kind: merge.HunkConflict, OursBytes: []byte("ours-B\n"), TheirsBytes: []byte("theirs-B\n")},
	}
	st, ed := newState(t), newEditor(t)
	st, ed, _, _ = Enter(input, st, ed)

	if !IsActive(st) {
		t.Fatal("expected merge active with 2 conflicts")
	}
	if n := ConflictsLeft(st); n != 2 {
		t.Fatalf("ConflictsLeft=%d, want 2", n)
	}
	if len(st.blocks) != 2 {
		t.Fatalf("blocks=%d, want 2", len(st.blocks))
	}
	if st.blocks[0].start >= st.blocks[1].start {
		t.Fatalf("blocks out of order: [0].start=%d >= [1].start=%d", st.blocks[0].start, st.blocks[1].start)
	}
	if st.blocks[0].end > st.blocks[1].start {
		t.Fatalf("blocks overlap: [0].end=%d > [1].start=%d", st.blocks[0].end, st.blocks[1].start)
	}
	_ = ed
}

// ─────────────────────────────────────────────────────────────────────────────
// [O] / [T] resolve a conflict — and BOTH are undoable/redoable journaled edits
// ─────────────────────────────────────────────────────────────────────────────

func TestHandleKey_AcceptOurs_ResolvesToOursBytes(t *testing.T) {
	const oursContent = "ours choice\n"
	st, ed := newState(t), newEditor(t)
	st, ed, _, _ = Enter(hunks(merge.HunkConflict, oursContent, "theirs choice\n"), st, ed)

	var consumed bool
	st, ed, _, consumed, _ = HandleKey(st, ed, press('o'))
	if !consumed {
		t.Fatal("[o]: expected consumed=true")
	}
	if IsActive(st) {
		t.Fatal("[O]: IsActive should be false after resolving the only conflict")
	}
	if got := ed.Content(); got != oursContent {
		t.Fatalf("[O]: buffer=%q, want %q (markers must be gone)", got, oursContent)
	}
}

func TestHandleKey_AcceptOurs_UpperCase(t *testing.T) {
	st, ed := newState(t), newEditor(t)
	st, ed, _, _ = Enter(hunks(merge.HunkConflict, "ours\n", "theirs\n"), st, ed)
	st, ed, _, _, _ = HandleKey(st, ed, press('O'))
	if HasUnresolvedConflicts(st) {
		t.Fatal("[O] uppercase: still has unresolved conflicts")
	}
	_ = ed
}

func TestHandleKey_AcceptTheirs_ResolvesToTheirsBytes(t *testing.T) {
	const oursContent = "ours choice\n"
	const theirsContent = "theirs choice\n"
	st, ed := newState(t), newEditor(t)
	st, ed, _, _ = Enter(hunks(merge.HunkConflict, oursContent, theirsContent), st, ed)

	st, ed, _, _, _ = HandleKey(st, ed, press('t'))

	if IsActive(st) {
		t.Fatal("[T]: IsActive should be false after resolving the only conflict")
	}
	if got := ed.Content(); got != theirsContent {
		t.Fatalf("[T]: buffer=%q, want %q", got, theirsContent)
	}
}

func TestHandleKey_AcceptTheirs_AdjustsSubsequentOffsets(t *testing.T) {
	oursA := "ours-A\n"
	theirsA := "theirs-A-longer\n" // expands the block
	oursB := "ours-B\n"
	theirsB := "theirs-B\n"

	input := []merge.Hunk{
		{Kind: merge.HunkConflict, OursBytes: []byte(oursA), TheirsBytes: []byte(theirsA)},
		{Kind: merge.HunkConflict, OursBytes: []byte(oursB), TheirsBytes: []byte(theirsB)},
	}
	st, ed := newState(t), newEditor(t)
	st, ed, _, _ = Enter(input, st, ed)

	st, ed, _, _, _ = HandleKey(st, ed, press('t')) // resolve block A → theirsA (delta +9)
	st, ed, _, _, _ = HandleKey(st, ed, press('t')) // resolve block B → theirsB (must use shifted span)

	if HasUnresolvedConflicts(st) {
		t.Fatal("still has unresolved conflicts after accepting both with [T]")
	}
	want := theirsA + theirsB
	if got := ed.Content(); got != want {
		t.Fatalf("buffer after [T][T]=%q, want %q", got, want)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// Undoability — the core requirement (§3): [O] and [T] are ordinary journaled
// ReplaceRanges, reversible via ApplyInverse and re-appliable via Reapply.
// ─────────────────────────────────────────────────────────────────────────────

func TestAcceptTheirs_DrainsNonEmptyEdits_UndoableRedoable(t *testing.T) {
	const oursContent = "ours original\n"
	const theirsContent = "theirs new\n"
	st, ed := newState(t), newEditor(t)
	st, ed, _, _ = Enter(hunks(merge.HunkConflict, oursContent, theirsContent), st, ed)

	// Drain Enter's own marker-load edit first.
	ed, _ = ed.DrainEdits()
	markerBuffer := ed.Content() // the well-formed marker buffer, for later comparison

	var cmd tea.Cmd
	st, ed, cmd, _, _ = HandleKey(st, ed, press('t'))
	_ = cmd
	if got := ed.Content(); got != theirsContent {
		t.Fatalf("after [T]: buffer=%q, want %q", got, theirsContent)
	}

	ed, edits := ed.DrainEdits()
	if len(edits) == 0 {
		t.Fatal("[T]: no pending edits drained — ReplaceRange not journaled (undo would be a no-op, §1.4/journal.go AppendEdit)")
	}

	// Undo: ApplyInverse must restore the marker buffer.
	ed, _, err := ed.ApplyInverse(edits)
	if err != nil {
		t.Fatalf("ApplyInverse: %v", err)
	}
	if got := ed.Content(); got != markerBuffer {
		t.Fatalf("after ApplyInverse: buffer=%q, want marker buffer %q", got, markerBuffer)
	}

	// Redo: Reapply must restore theirsContent again.
	ed, _, err = ed.Reapply(edits)
	if err != nil {
		t.Fatalf("Reapply: %v", err)
	}
	if got := ed.Content(); got != theirsContent {
		t.Fatalf("after Reapply: buffer=%q, want %q", got, theirsContent)
	}
}

func TestAcceptOurs_DrainsNonEmptyEdits_UndoableRedoable(t *testing.T) {
	const oursContent = "ours kept\n"
	const theirsContent = "theirs alt\n"
	st, ed := newState(t), newEditor(t)
	st, ed, _, _ = Enter(hunks(merge.HunkConflict, oursContent, theirsContent), st, ed)

	ed, _ = ed.DrainEdits()
	markerBuffer := ed.Content()

	st, ed, _, _, _ = HandleKey(st, ed, press('o'))
	if got := ed.Content(); got != oursContent {
		t.Fatalf("after [O]: buffer=%q, want %q", got, oursContent)
	}

	ed, edits := ed.DrainEdits()
	if len(edits) == 0 {
		t.Fatal("[O]: no pending edits drained — [O] must be an ordinary journaled ReplaceRange, not a flag flip (§3 core requirement)")
	}

	ed, _, err := ed.ApplyInverse(edits)
	if err != nil {
		t.Fatalf("ApplyInverse: %v", err)
	}
	if got := ed.Content(); got != markerBuffer {
		t.Fatalf("after ApplyInverse: buffer=%q, want marker buffer %q", got, markerBuffer)
	}

	ed, _, err = ed.Reapply(edits)
	if err != nil {
		t.Fatalf("Reapply: %v", err)
	}
	if got := ed.Content(); got != oursContent {
		t.Fatalf("after Reapply: buffer=%q, want %q", got, oursContent)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// Text input is blocked; navigation/scroll keys are consumed without mutating
// ─────────────────────────────────────────────────────────────────────────────

func TestHandleKey_TextInputBlocked(t *testing.T) {
	const oursContent = "ours\n"
	st, ed := newState(t), newEditor(t)
	st, ed, _, _ = Enter(hunks(merge.HunkConflict, oursContent, "theirs\n"), st, ed)
	before := ed.Content()

	var consumed bool
	st, ed, _, consumed, _ = HandleKey(st, ed, press('x'))
	if !consumed {
		t.Fatal("stray key while merging must be consumed (no free editing, §3)")
	}
	if got := ed.Content(); got != before {
		t.Fatalf("text input not blocked in merge mode: buffer=%q, want unchanged %q", got, before)
	}
}

func TestHandleKey_ScrollKeysDoNotMutateOrPanic(t *testing.T) {
	st, ed := newState(t), newEditor(t)
	st, ed, _, _ = Enter(hunks(merge.HunkConflict, "ours\n", "theirs\n"), st, ed)
	before := ed.Content()

	for _, code := range []rune{tea.KeyUp, tea.KeyDown, tea.KeyPgUp, tea.KeyPgDown, tea.KeyHome, tea.KeyEnd} {
		st, ed, _, _, _ = HandleKey(st, ed, tea.KeyPressMsg{Code: code})
	}
	if !HasUnresolvedConflicts(st) {
		t.Fatal("scroll keys resolved a conflict — they should not")
	}
	if got := ed.Content(); got != before {
		t.Fatalf("scroll keys mutated the buffer: got %q, want %q", got, before)
	}
}

func TestHandleKey_NextPrevMovesCurrentWithoutResolving(t *testing.T) {
	input := []merge.Hunk{
		{Kind: merge.HunkConflict, OursBytes: []byte("A\n"), TheirsBytes: []byte("a\n")},
		{Kind: merge.HunkConflict, OursBytes: []byte("B\n"), TheirsBytes: []byte("b\n")},
	}
	st, ed := newState(t), newEditor(t)
	st, ed, _, _ = Enter(input, st, ed)

	if st.cur != 0 {
		t.Fatalf("cur=%d, want 0 (first unresolved)", st.cur)
	}
	st, ed, _, _, _ = HandleKey(st, ed, press('n'))
	if st.cur != 1 {
		t.Fatalf("[n]: cur=%d, want 1", st.cur)
	}
	st, ed, _, _, _ = HandleKey(st, ed, press('p'))
	if st.cur != 0 {
		t.Fatalf("[p]: cur=%d, want 0", st.cur)
	}
	if ConflictsLeft(st) != 2 {
		t.Fatal("[n]/[p] must not resolve any conflict")
	}
	_ = ed
}

// ─────────────────────────────────────────────────────────────────────────────
// ConflictsLeft
// ─────────────────────────────────────────────────────────────────────────────

func TestConflictsLeft_ZeroWhenInactive(t *testing.T) {
	st := newState(t)
	if n := ConflictsLeft(st); n != 0 {
		t.Fatalf("ConflictsLeft on zero-value-ish State: got %d, want 0", n)
	}
}

func TestConflictsLeft_DecrementsPerAccept(t *testing.T) {
	input := []merge.Hunk{
		{Kind: merge.HunkConflict, OursBytes: []byte("A\n"), TheirsBytes: []byte("a\n")},
		{Kind: merge.HunkConflict, OursBytes: []byte("B\n"), TheirsBytes: []byte("b\n")},
	}
	st, ed := newState(t), newEditor(t)
	st, ed, _, _ = Enter(input, st, ed)
	if n := ConflictsLeft(st); n != 2 {
		t.Fatalf("ConflictsLeft=%d, want 2", n)
	}
	st, ed, _, _, _ = HandleKey(st, ed, press('o'))
	if n := ConflictsLeft(st); n != 1 {
		t.Fatalf("ConflictsLeft after one accept=%d, want 1", n)
	}
	st, ed, _, _, _ = HandleKey(st, ed, press('t'))
	if n := ConflictsLeft(st); n != 0 {
		t.Fatalf("ConflictsLeft after both accepted=%d, want 0", n)
	}
	if IsActive(st) {
		t.Fatal("merge must be inactive once all conflicts are resolved")
	}
}
