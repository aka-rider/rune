package docstate

import (
	"testing"
	"time"

	"rune/pkg/editor/buffer"
)

// This file holds the two WP7 regressions for a shared root cause the fuzz
// driver's new "store.Content == live buffer" property discovered: a
// snapshot anchored at a journal position that is later mutated (coalescing)
// or abandoned (undo truncation) becomes a stale/zombie anchor
// RecoverDocument can still pick, resurrecting bytes that should never
// reappear. Extracted from journal_test.go to keep that file under the
// 500-LoC limit (§1.6/§11).

// TestAppendEdit_NeverCoalescesIntoSnapshottedSeq is the WP7 regression for a
// bug the "store.Content == live buffer at every step" fuzz property (b)
// discovered: a snapshot taken at seq=N freezes "content up to and including
// N" as of that moment; RecoverDocument's replay window is
// seq > anchorSnapshotSeq AND seq <= targetSeq, which — when the anchor's own
// seq equals N — never revisits the row AT N. Coalescing a later single-char
// insert into that SAME row (an UPDATE, not a new INSERT) after the snapshot
// exists silently orphans the coalesced bytes from any snapshot-anchored
// reconstruction, even though the events row itself now holds them: a
// RecoverDocument call (crash recovery, Load's reopen-with-history path)
// would reconstruct pre-coalesce content, silently dropping the coalesced
// character. Confirmed via git-stash of the AppendEdit fix: this test fails
// on the pre-fix tree (got "abc", want "abcd") and passes after.
func TestAppendEdit_NeverCoalescesIntoSnapshottedSeq(t *testing.T) {
	s := NewTestStore(t)
	docID := testDoc(t, s)

	now := time.Now()
	s.clock = func() time.Time { return now }

	if _, err := s.AppendEdit(docID, singleInsert("a"), noCursors, noCursors); err != nil {
		t.Fatalf("AppendEdit a: %v", err)
	}
	seq, err := s.CurrentSeq(docID)
	if err != nil {
		t.Fatalf("CurrentSeq: %v", err)
	}

	// A debounced flush (or ⌘S's own snapshot path) anchors a recovery
	// snapshot at THIS seq, capturing "a" — exactly what a freeze-at-flush
	// cadence does mid-typing.
	if _, err := s.CreateSnapshot(docID, "a", seq); err != nil {
		t.Fatalf("CreateSnapshot: %v", err)
	}

	// A second single-char insert, well within the coalesce window, would
	// normally coalesce into the SAME row (seq unchanged) — but a snapshot
	// now anchors that exact seq, so it must NOT.
	now = now.Add(50 * time.Millisecond)
	if _, err := s.AppendEdit(docID, singleInsert("b"), noCursors, noCursors); err != nil {
		t.Fatalf("AppendEdit b: %v", err)
	}

	if n := countRows(t, s.perm, `SELECT COUNT(*) FROM events WHERE doc_id=?`, docID); n != 2 {
		t.Errorf("expected 2 event rows (coalesce refused at a snapshotted seq), got %d", n)
	}
	got, err := s.Content(docID)
	if err != nil {
		t.Fatalf("Content: %v", err)
	}
	// singleInsert always targets Start:0,End:0 (position-0 insert, matching
	// TestCoalescingWithinWindow's own helper semantics) — replaying "a" then
	// "b" both at position 0 correctly produces "ba" (b pushes a to
	// position 1). The point of this assertion is that BOTH characters
	// survive reconstruction — pre-fix this returns "a" alone (b silently
	// dropped by the stale snapshot anchor).
	if got != "ba" {
		t.Fatalf("Content: got %q, want %q — the second insert was silently lost from a snapshot-anchored reconstruction", got, "ba")
	}
}

// TestAppendEdit_TruncationInvalidatesFutureSnapshots is the WP7 regression
// for a second, deeper instance of the SAME root cause the fuzz driver's new
// properties found (via FuzzHumanSession's globalSeqDirtySpec workflow, cluster
// 7, mutated with an appended dirtyCloseGuard cluster — corpus entry
// "(0"/bbffbee953abbb79): AppendEdit's "truncate abandoned future" step
// deletes EVENTS past the undo position but never invalidated SNAPSHOTS taken
// within that abandoned future. RecoverDocument picks "nearest snapshot AT OR
// BEFORE targetSeq" as its anchor — once a NEW edit lands at a fresh
// (AUTOINCREMENT) seq after the undo, a snapshot from the truncated-away
// future can still be seq-eligible and gets picked, resurrecting abandoned
// content UNDER the new edit instead of reconstructing the new edit alone.
// Confirmed via git-stash of the AppendEdit fix: this test fails on the
// pre-fix tree (got "dirtyfile-b-edit-1", want "dirty") and passes after.
func TestAppendEdit_TruncationInvalidatesFutureSnapshots(t *testing.T) {
	s := NewTestStore(t)
	docID := testDoc(t, s)

	now := time.Now()
	s.clock = func() time.Time { return now }

	// Type a multi-char insert (avoids single-char coalescing entirely, like
	// a paste/IME commit or the fuzz workflow's own textEvent).
	if _, err := s.AppendEdit(docID, textInsert("file-b-edit-1"), noCursors, noCursors); err != nil {
		t.Fatalf("AppendEdit edit-1: %v", err)
	}
	seq1, err := s.CurrentSeq(docID)
	if err != nil {
		t.Fatalf("CurrentSeq: %v", err)
	}
	// A debounced flush anchors a recovery snapshot mid-typing, at seq1.
	if _, err := s.CreateSnapshot(docID, "file-b-edit-1", seq1); err != nil {
		t.Fatalf("CreateSnapshot: %v", err)
	}

	now = now.Add(400 * time.Millisecond)
	if _, err := s.AppendEdit(docID, textInsert("file-b-edit-2"), noCursors, noCursors); err != nil {
		t.Fatalf("AppendEdit edit-2: %v", err)
	}

	// Undo both edits — abandon this entire future, back to position 0.
	for i := 0; i < 2; i++ {
		step, ok, uerr := s.UndoPeek(docID)
		if uerr != nil {
			t.Fatalf("UndoPeek: %v", uerr)
		}
		if !ok {
			break
		}
		if uerr := s.MoveUndoPos(docID, step.NewPos); uerr != nil {
			t.Fatalf("MoveUndoPos: %v", uerr)
		}
	}

	now = now.Add(400 * time.Millisecond)
	// A fresh edit after undoing past the snapshot's own seq — this is what
	// must invalidate (delete) the now-unreachable snapshot, not just the
	// events that led to it.
	if _, err := s.AppendEdit(docID, textInsert("dirty"), noCursors, noCursors); err != nil {
		t.Fatalf("AppendEdit dirty: %v", err)
	}

	if n := countRows(t, s.perm, `SELECT COUNT(*) FROM snapshots WHERE doc_id=? AND seq > 0`, docID); n != 0 {
		t.Errorf("expected the seq=%d snapshot to be invalidated by truncation, got %d snapshots past seq 0", seq1, n)
	}
	got, err := s.Content(docID)
	if err != nil {
		t.Fatalf("Content: %v", err)
	}
	if got != "dirty" {
		t.Fatalf("Content: got %q, want %q — a snapshot from the abandoned future was resurrected as the replay anchor", got, "dirty")
	}
}

// textInsert returns a multi-character AppliedEdit (never coalesces via
// isInsertChar's single-rune check) — used where the test needs to isolate
// truncation/snapshot behavior from coalescing.
func textInsert(s string) []buffer.AppliedEdit {
	return []buffer.AppliedEdit{{Start: 0, End: 0, Deleted: "", Insert: s}}
}
