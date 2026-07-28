package docstate

import (
	"testing"
	"time"
)

// TestEditsInRange_PlainRange verifies the basic (fromSeq, toSeq] contract:
// ordered ascending, each row tagged with its own seq, bounds respected.
func TestEditsInRange_PlainRange(t *testing.T) {
	s := NewTestStore(t)
	docID := testDoc(t, s)

	advance := fixedClock(s)

	// Three edits, each outside the coalescing window so each gets its own row.
	if _, err := s.AppendEdit(docID, textInsert("a"), noCursors, noCursors); err != nil {
		t.Fatalf("AppendEdit a: %v", err)
	}
	seqA, err := s.CurrentSeq(docID)
	if err != nil {
		t.Fatalf("CurrentSeq after a: %v", err)
	}

	advance(400 * time.Millisecond)
	if _, err := s.AppendEdit(docID, textInsert("b"), noCursors, noCursors); err != nil {
		t.Fatalf("AppendEdit b: %v", err)
	}
	seqB, err := s.CurrentSeq(docID)
	if err != nil {
		t.Fatalf("CurrentSeq after b: %v", err)
	}

	advance(400 * time.Millisecond)
	if _, err := s.AppendEdit(docID, textInsert("c"), noCursors, noCursors); err != nil {
		t.Fatalf("AppendEdit c: %v", err)
	}
	seqC, err := s.CurrentSeq(docID)
	if err != nil {
		t.Fatalf("CurrentSeq after c: %v", err)
	}

	// Full range: all three rows, ascending, correctly tagged.
	rows, err := s.EditsInRange(docID, 0, seqC)
	if err != nil {
		t.Fatalf("EditsInRange(0, seqC): %v", err)
	}
	if len(rows) != 3 {
		t.Fatalf("EditsInRange(0, seqC): got %d rows, want 3", len(rows))
	}
	wantSeqs := []int64{seqA, seqB, seqC}
	wantInserts := []string{"a", "b", "c"}
	for i, row := range rows {
		if row.Seq != wantSeqs[i] {
			t.Errorf("row %d: Seq=%d, want %d", i, row.Seq, wantSeqs[i])
		}
		if len(row.Edits) != 1 || row.Edits[0].Insert != wantInserts[i] {
			t.Errorf("row %d: Edits=%+v, want single insert %q", i, row.Edits, wantInserts[i])
		}
	}

	// Bounded range: strictly after seqA, up to and including seqB — excludes
	// both a (fromSeq boundary is exclusive) and c (beyond toSeq).
	mid, err := s.EditsInRange(docID, seqA, seqB)
	if err != nil {
		t.Fatalf("EditsInRange(seqA, seqB): %v", err)
	}
	if len(mid) != 1 || mid[0].Seq != seqB || mid[0].Edits[0].Insert != "b" {
		t.Fatalf("EditsInRange(seqA, seqB): got %+v, want exactly [b]@seqB", mid)
	}

	// Empty range: fromSeq == toSeq.
	empty, err := s.EditsInRange(docID, seqB, seqB)
	if err != nil {
		t.Fatalf("EditsInRange(seqB, seqB): %v", err)
	}
	if len(empty) != 0 {
		t.Fatalf("EditsInRange(seqB, seqB): got %d rows, want 0", len(empty))
	}
}

// TestEditsInRange_ReflectsCoalesce verifies that a row fetched via
// EditsInRange reflects AppendEdit's keystroke-coalescing UPDATE — i.e. the
// row's content is whatever it currently holds, not whatever it held at
// first insert. This is the exact property the fuzz driver's mirrorFor cache
// depends on: it must never trust a cached copy of the current tail row,
// because that row's content can still change in place.
func TestEditsInRange_ReflectsCoalesce(t *testing.T) {
	s := NewTestStore(t)
	docID := testDoc(t, s)

	advance := fixedClock(s)

	if _, err := s.AppendEdit(docID, singleInsert("h"), noCursors, noCursors); err != nil {
		t.Fatalf("AppendEdit h: %v", err)
	}
	seq1, err := s.CurrentSeq(docID)
	if err != nil {
		t.Fatalf("CurrentSeq: %v", err)
	}

	// A fetch right after the first keystroke sees just "h".
	rows, err := s.EditsInRange(docID, 0, seq1)
	if err != nil {
		t.Fatalf("EditsInRange (before coalesce): %v", err)
	}
	if len(rows) != 1 || rows[0].Edits[0].Insert != "h" {
		t.Fatalf("EditsInRange (before coalesce): got %+v, want single insert \"h\"", rows)
	}

	// Second keystroke within the coalescing window, continuing the typing
	// run (starts where "h" ended): same seq, mutated row.
	advance(50 * time.Millisecond)
	if _, err := s.AppendEdit(docID, insertAt(1, "e"), noCursors, noCursors); err != nil {
		t.Fatalf("AppendEdit e: %v", err)
	}
	seq2, err := s.CurrentSeq(docID)
	if err != nil {
		t.Fatalf("CurrentSeq after coalesce: %v", err)
	}
	if seq2 != seq1 {
		t.Fatalf("expected coalescing to keep seq unchanged: seq1=%d seq2=%d", seq1, seq2)
	}

	// Refetching the SAME seq must now reflect the coalesced content, not the
	// stale pre-coalesce "h" alone.
	rows, err = s.EditsInRange(docID, 0, seq2)
	if err != nil {
		t.Fatalf("EditsInRange (after coalesce): %v", err)
	}
	if len(rows) != 1 {
		t.Fatalf("EditsInRange (after coalesce): got %d rows, want 1 (coalesced, no new row)", len(rows))
	}
	got, err := s.Content(docID)
	if err != nil {
		t.Fatalf("Content: %v", err)
	}
	if got != "he" {
		t.Fatalf("Content: got %q, want %q (both keystrokes must survive the coalesce)", got, "he")
	}
}

// TestEditsInRange_TruncationRemovesAbandonedRows verifies that once
// AppendEdit truncates an abandoned future (edit after undo), a range query
// spanning the deleted seqs no longer returns them — the fast-path cache in
// mirrorFor depends on this: it must never resurrect truncated rows.
func TestEditsInRange_TruncationRemovesAbandonedRows(t *testing.T) {
	s := NewTestStore(t)
	docID := testDoc(t, s)

	advance := fixedClock(s)

	if _, err := s.AppendEdit(docID, textInsert("file-edit-1"), noCursors, noCursors); err != nil {
		t.Fatalf("AppendEdit edit-1: %v", err)
	}
	advance(400 * time.Millisecond)
	if _, err := s.AppendEdit(docID, textInsert("file-edit-2"), noCursors, noCursors); err != nil {
		t.Fatalf("AppendEdit edit-2: %v", err)
	}
	seqBeforeUndo, err := s.CurrentSeq(docID)
	if err != nil {
		t.Fatalf("CurrentSeq: %v", err)
	}

	// Undo both edits back to position 0.
	for range 2 {
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

	advance(400 * time.Millisecond)
	// A fresh edit after the undo truncates the abandoned future and lands at
	// a brand-new (higher) seq — AUTOINCREMENT never reuses the deleted seqs.
	if _, err := s.AppendEdit(docID, textInsert("dirty"), noCursors, noCursors); err != nil {
		t.Fatalf("AppendEdit dirty: %v", err)
	}
	seqAfter, err := s.CurrentSeq(docID)
	if err != nil {
		t.Fatalf("CurrentSeq after dirty: %v", err)
	}
	if seqAfter <= seqBeforeUndo {
		t.Fatalf("expected the new edit's seq (%d) to exceed the truncated seq (%d)", seqAfter, seqBeforeUndo)
	}

	// A range spanning the OLD (now-deleted) seqs through the new one must
	// return only the surviving row — the truncated rows never reappear.
	rows, err := s.EditsInRange(docID, 0, seqAfter)
	if err != nil {
		t.Fatalf("EditsInRange after truncation: %v", err)
	}
	if len(rows) != 1 || rows[0].Seq != seqAfter || rows[0].Edits[0].Insert != "dirty" {
		t.Fatalf("EditsInRange after truncation: got %+v, want exactly [dirty]@%d", rows, seqAfter)
	}
}
