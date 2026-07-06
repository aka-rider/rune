package mergemode

import (
	"testing"

	"rune/pkg/merge"
)

// ─────────────────────────────────────────────────────────────────────────────
// Abort — the Esc escape hatch
// ─────────────────────────────────────────────────────────────────────────────

func TestAbort_RevertsToExactPreMergeOurs(t *testing.T) {
	preMergeOurs := "line one\nline two\n"
	ed := newEditor(t)
	ed = ed.SetContent(preMergeOurs)
	st := newState(t)

	hunkSet := []merge.Hunk{
		{Kind: merge.HunkClean, AutoBytes: []byte("line one\n")},
		{Kind: merge.HunkConflict, OursBytes: []byte("line two\n"), TheirsBytes: []byte("line two (theirs)\n")},
	}
	st, ed, _, _ = Enter(hunkSet, st, ed)
	if !IsActive(st) {
		t.Fatal("expected active merge before Abort")
	}

	// Resolve nothing — abort immediately from mid-merge.
	st, ed, _, _ = Abort(st, ed)

	if IsActive(st) {
		t.Fatal("Abort must deactivate the merge")
	}
	if got := ed.Content(); got != preMergeOurs {
		t.Fatalf("Abort: buffer=%q, want exact pre-merge ours %q", got, preMergeOurs)
	}
}

func TestAbort_AfterPartialResolution_StillRevertsToPreMergeOurs(t *testing.T) {
	preMergeOurs := "A\nB\nC\n"
	ed := newEditor(t)
	ed = ed.SetContent(preMergeOurs)
	st := newState(t)

	hunkSet := []merge.Hunk{
		{Kind: merge.HunkConflict, OursBytes: []byte("A\n"), TheirsBytes: []byte("a\n")},
		{Kind: merge.HunkClean, AutoBytes: []byte("B\n")},
		{Kind: merge.HunkConflict, OursBytes: []byte("C\n"), TheirsBytes: []byte("c\n")},
	}
	st, ed, _, _ = Enter(hunkSet, st, ed)
	st, ed, _, _, _ = HandleKey(st, ed, press('t')) // resolve first block

	st, ed, _, _ = Abort(st, ed)
	if got := ed.Content(); got != preMergeOurs {
		t.Fatalf("Abort after partial resolution: buffer=%q, want %q", got, preMergeOurs)
	}
	if IsActive(st) {
		t.Fatal("Abort must deactivate")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// Reset
// ─────────────────────────────────────────────────────────────────────────────

func TestReset_ClearsActiveState(t *testing.T) {
	st, ed := newState(t), newEditor(t)
	st, ed, _, _ = Enter(hunks(merge.HunkConflict, "ours\n", "theirs\n"), st, ed)
	if !IsActive(st) {
		t.Fatal("expected active before Reset")
	}
	st = Reset(st)
	if IsActive(st) {
		t.Fatal("Reset must clear active")
	}
	if ConflictsLeft(st) != 0 {
		t.Fatal("Reset must clear the conflict list")
	}
	_ = ed
}

// ─────────────────────────────────────────────────────────────────────────────
// Resync — slot-ordered AND content-verifying (critic R1)
// ─────────────────────────────────────────────────────────────────────────────

// TestResync_MatchesEnterState: calling Resync right after Enter (no edits at
// all) must reconstruct the exact same block spans Enter itself computed —
// the baseline sanity check before testing adversarial content below.
func TestResync_MatchesEnterState(t *testing.T) {
	input := []merge.Hunk{
		{Kind: merge.HunkClean, AutoBytes: []byte("header\n")},
		{Kind: merge.HunkConflict, OursBytes: []byte("ours-A\n"), TheirsBytes: []byte("theirs-A\n")},
		{Kind: merge.HunkClean, AutoBytes: []byte("middle\n")},
		{Kind: merge.HunkConflict, OursBytes: []byte("ours-B\n"), TheirsBytes: []byte("theirs-B\n")},
	}
	st, ed := newState(t), newEditor(t)
	st, ed, _, _ = Enter(input, st, ed)
	wantBlocks := append([]block(nil), st.blocks...)

	resynced := Resync(st, ed)
	if len(resynced.blocks) != len(wantBlocks) {
		t.Fatalf("Resync: %d blocks, want %d", len(resynced.blocks), len(wantBlocks))
	}
	for i, b := range resynced.blocks {
		if b != wantBlocks[i] {
			t.Fatalf("Resync block[%d]=%+v, want %+v", i, b, wantBlocks[i])
		}
	}
	if !IsActive(resynced) {
		t.Fatal("Resync: expected still active")
	}
}

// TestResync_RejectsSpuriousMarkersInContent: ours/theirs content containing a
// literal "<<<<<<<" / "=======" / ">>>>>>>" line (not just "=======") must
// still map every block to the correct immutable side and resolve to exact
// bytes — the verifying scan must reject the spurious anchors (critic R1).
func TestResync_RejectsSpuriousMarkersInContent(t *testing.T) {
	spuriousOurs := "a quoted example:\n<<<<<<< fake\nfake ours\n=======\nfake theirs\n>>>>>>> fake\nend\n"
	realTheirs := "theirs replacement\n"

	input := []merge.Hunk{
		{Kind: merge.HunkClean, AutoBytes: []byte("intro\n")},
		{Kind: merge.HunkConflict, OursBytes: []byte(spuriousOurs), TheirsBytes: []byte(realTheirs)},
		{Kind: merge.HunkClean, AutoBytes: []byte("outro\n")},
	}
	st, ed := newState(t), newEditor(t)
	st, ed, _, _ = Enter(input, st, ed)

	if len(st.blocks) != 1 {
		t.Fatalf("expected 1 conflict block, got %d", len(st.blocks))
	}

	// Resync must reconstruct the SAME single block (not fooled by the fake
	// anchors embedded in oursBytes) — content-verifying, not anchor-counting.
	resynced := Resync(st, ed)
	if len(resynced.blocks) != 1 {
		t.Fatalf("Resync: %d blocks, want 1", len(resynced.blocks))
	}
	if resynced.blocks[0] != st.blocks[0] {
		t.Fatalf("Resync misdetected the block: got %+v, want %+v", resynced.blocks[0], st.blocks[0])
	}
	if !HasUnresolvedConflicts(resynced) {
		t.Fatal("Resync: expected the real conflict to still be unresolved")
	}

	// Now accept theirs and Resync again — the resolved-detection must also
	// not be fooled by the fake markers baked into oursBytes (which are no
	// longer in the buffer at all once theirs is accepted).
	st, ed, _, _, _ = HandleKey(st, ed, press('t'))
	if got := ed.Content(); got != "intro\n"+realTheirs+"outro\n" {
		t.Fatalf("after [T]: buffer=%q", got)
	}
	resynced = Resync(st, ed)
	if IsActive(resynced) {
		t.Fatal("Resync after full resolution: expected inactive")
	}
}

// TestResync_TwoIdenticalConflicts_OneResolved: E4 — two conflicts with
// byte-identical ours AND byte-identical theirs are disambiguated only by
// slot order + the monotonic searchFrom, which is documented (not fixed) as
// ambiguous in the general case: resolving the FIRST identical conflict can
// make Resync attribute the still-framed SECOND block's bytes to the wrong
// slot internally. This is provably harmless here specifically BECAUSE the
// two conflicts are byte-identical — whichever slot's ours/theirs Resync
// (mis)attributes a block to, the bytes it holds are the same either way. The
// invariant that actually matters and must hold: exactly one conflict remains
// after resolving the first, and resolving it (however Resync mapped it)
// yields the exact correct final bytes with merge deactivating cleanly —
// never wrong bytes, never a stuck/lost conflict.
func TestResync_TwoIdenticalConflicts_OneResolved(t *testing.T) {
	const ours = "same ours\n"
	const theirs = "same theirs\n"
	input := []merge.Hunk{
		{Kind: merge.HunkConflict, OursBytes: []byte(ours), TheirsBytes: []byte(theirs)},
		{Kind: merge.HunkClean, AutoBytes: []byte("---\n")},
		{Kind: merge.HunkConflict, OursBytes: []byte(ours), TheirsBytes: []byte(theirs)},
	}
	st, ed := newState(t), newEditor(t)
	st, ed, _, _ = Enter(input, st, ed)
	if n := ConflictsLeft(st); n != 2 {
		t.Fatalf("setup: ConflictsLeft=%d, want 2", n)
	}

	// Resolve ONLY the first block to ours; the second stays framed.
	st, ed, _, _, _ = HandleKey(st, ed, press('o'))
	if n := ConflictsLeft(st); n != 1 {
		t.Fatalf("after resolving block 0: ConflictsLeft=%d, want 1", n)
	}
	wantAfterFirstAccept := ours + "---\n" + string(frameBlock([]byte(ours), []byte(theirs)))
	if got := ed.Content(); got != wantAfterFirstAccept {
		t.Fatalf("after [O] on block 0: buffer=%q, want %q", got, wantAfterFirstAccept)
	}

	// Resync must still see exactly ONE unresolved conflict — never zero
	// (silently losing the still-framed block) and never two (misreading the
	// resolved form as still-conflicted).
	resynced := Resync(st, ed)
	if !HasUnresolvedConflicts(resynced) {
		t.Fatal("Resync: the still-framed second block must read unresolved")
	}
	if n := ConflictsLeft(resynced); n != 1 {
		t.Fatalf("Resync: ConflictsLeft=%d, want 1", n)
	}

	// Resolving the remaining conflict must produce the EXACT correct final
	// bytes and fully deactivate — regardless of which internal slot Resync
	// attributed the still-framed block to (documented ambiguity, harmless
	// here because both conflicts are byte-identical).
	resynced, ed, _, consumed, _ := HandleKey(resynced, ed, press('t'))
	if !consumed {
		t.Fatal("[t]: expected consumed=true")
	}
	want := ours + "---\n" + theirs
	if got := ed.Content(); got != want {
		t.Fatalf("after resolving both identical conflicts: buffer=%q, want %q", got, want)
	}
	if IsActive(resynced) {
		t.Fatal("expected merge inactive once both identical conflicts are resolved")
	}
	if HasUnresolvedConflicts(resynced) {
		t.Fatal("expected zero unresolved conflicts once both are resolved")
	}
}

// TestResync_UndoPastEnter_NoBlocksFound: undoing all the way back to the
// pre-merge buffer (no markers, no resolved forms present — the ORIGINAL
// document) must resync to zero unresolved blocks so the caller exits merge.
func TestResync_UndoPastEnter_NoBlocksFound(t *testing.T) {
	preMergeOurs := "only ours, never merged\n"
	ed := newEditor(t)
	ed = ed.SetContent(preMergeOurs)
	st := newState(t)

	st, ed, _, _ = Enter(hunks(merge.HunkConflict, preMergeOurs, "theirs\n"), st, ed)
	if !IsActive(st) {
		t.Fatal("expected active after Enter")
	}

	// Simulate "undo past Enter": restore the buffer to the exact pre-merge
	// content (as ApplyInverse of the marker-load edit would).
	ed = ed.SetContent(preMergeOurs)

	resynced := Resync(st, ed)
	if IsActive(resynced) {
		t.Fatal("Resync on the pre-merge buffer must exit merge mode (no blocks found)")
	}
}
