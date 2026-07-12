package docstate

import (
	"testing"
	"time"

	"rune/pkg/editor/buffer"
	"rune/pkg/editor/cursor"
)

// ---- P1: undo/redo correctness (journal) ------------------------------------

func TestBasicUndo(t *testing.T) {
	s := NewTestStore(t)
	docID := testDoc(t, s)

	edits := singleInsert("a")
	before := []cursor.Cursor{{Position: 0, Anchor: 0}}
	after := []cursor.Cursor{{Position: 1, Anchor: 1}}

	if _, err := s.AppendEdit(docID, edits, before, after); err != nil {
		t.Fatalf("AppendEdit: %v", err)
	}

	step, ok := undoStep(t, s, docID)
	if !ok {
		t.Fatal("UndoTarget: expected ok=true, got false")
	}
	if len(step.Edits) != 1 || step.Edits[0].Insert != "a" {
		t.Errorf("edits: got %+v, want insert=a", step.Edits)
	}
	if len(step.Cursors) != 1 || step.Cursors[0].Position != 0 {
		t.Errorf("cursorsBefore: got %+v, want position=0", step.Cursors)
	}

	// Second undo: nothing left.
	_, ok2 := undoStep(t, s, docID)
	if ok2 {
		t.Error("second UndoTarget: expected ok=false (nothing left to undo)")
	}
}

func TestBasicRedo(t *testing.T) {
	s := NewTestStore(t)
	docID := testDoc(t, s)

	edits := singleInsert("b")
	after := []cursor.Cursor{{Position: 1, Anchor: 1}}

	if _, err := s.AppendEdit(docID, edits, noCursors, after); err != nil {
		t.Fatalf("AppendEdit: %v", err)
	}

	// Undo.
	if _, ok := undoStep(t, s, docID); !ok {
		t.Fatal("UndoTarget returned ok=false")
	}

	// Redo.
	step, ok := redoStep(t, s, docID)
	if !ok {
		t.Fatal("RedoTarget: expected ok=true, got false")
	}
	if len(step.Edits) != 1 || step.Edits[0].Insert != "b" {
		t.Errorf("edits: got %+v, want insert=b", step.Edits)
	}
	if len(step.Cursors) != 1 || step.Cursors[0].Position != 1 {
		t.Errorf("cursorsAfter: got %+v, want position=1", step.Cursors)
	}

	// Second redo: nothing left.
	_, ok2 := redoStep(t, s, docID)
	if ok2 {
		t.Error("second RedoTarget: expected ok=false (nothing left to redo)")
	}
}

// TestUndoUndoRedo verifies that the journal correctly returns a redo target
// after two consecutive undos. This is the journal-layer counterpart to the
// Reapply regression test in the textedit package.
func TestUndoUndoRedo(t *testing.T) {
	s := NewTestStore(t)
	docID := testDoc(t, s)

	advance := fixedClock(s)

	// Event A.
	if _, err := s.AppendEdit(docID, singleInsert("a"), noCursors, noCursors); err != nil {
		t.Fatalf("AppendEdit A: %v", err)
	}
	// Event B (outside 300ms window → separate undo-stop).
	advance(400 * time.Millisecond)
	if _, err := s.AppendEdit(docID, singleInsert("b"), noCursors, noCursors); err != nil {
		t.Fatalf("AppendEdit B: %v", err)
	}

	// Undo B.
	if _, ok := undoStep(t, s, docID); !ok {
		t.Fatal("first UndoTarget returned ok=false")
	}
	// Undo A.
	stepA, ok := undoStep(t, s, docID)
	if !ok {
		t.Fatal("second UndoTarget returned ok=false")
	}
	if len(stepA.Edits) == 0 || stepA.Edits[0].Insert != "a" {
		t.Errorf("second undo edits: got %+v, want insert=a", stepA.Edits)
	}

	// Redo A — journal must return ok=true.
	redoA, okRedo := redoStep(t, s, docID)
	if !okRedo {
		t.Fatal("RedoTarget after Undo×2: expected ok=true, got false")
	}
	if len(redoA.Edits) == 0 || redoA.Edits[0].Insert != "a" {
		t.Errorf("redo edits: got %+v, want insert=a", redoA.Edits)
	}
}

func TestCoalescingWithinWindow(t *testing.T) {
	s := NewTestStore(t)
	docID := testDoc(t, s)
	advance := fixedClock(s)

	seq1, err := s.AppendEdit(docID, insertAt(0, "a"), noCursors, noCursors)
	if err != nil {
		t.Fatalf("AppendEdit 1: %v", err)
	}
	// Advance clock by 100ms (within 300ms window); 'b' continues the typing
	// run (starts where 'a' ended).
	advance(100 * time.Millisecond)
	seq2, err := s.AppendEdit(docID, insertAt(1, "b"), noCursors, noCursors)
	if err != nil {
		t.Fatalf("AppendEdit 2: %v", err)
	}

	// Behavior, not row counts: a coalesced append lands on the SAME journal
	// seq, and one undo step covers both inserts (one \u2318Z stop).
	if seq2 != seq1 {
		t.Errorf("expected coalesce into the same journal stop: seq1=%d seq2=%d", seq1, seq2)
	}
	step, ok := undoStep(t, s, docID)
	if !ok {
		t.Fatal("UndoPeek returned ok=false")
	}
	if len(step.Edits) != 2 {
		t.Errorf("one undo stop must cover both coalesced inserts: got %d edits", len(step.Edits))
	}
	if _, ok := undoStep(t, s, docID); ok {
		t.Error("expected exactly one undo stop after coalescing")
	}
}

func TestCoalescingOutsideWindow(t *testing.T) {
	s := NewTestStore(t)
	docID := testDoc(t, s)
	advance := fixedClock(s)

	seq1, err := s.AppendEdit(docID, insertAt(0, "a"), noCursors, noCursors)
	if err != nil {
		t.Fatalf("AppendEdit 1: %v", err)
	}
	// Advance clock by 400ms (outside 300ms window) — adjacency alone must
	// not coalesce.
	advance(400 * time.Millisecond)
	seq2, err := s.AppendEdit(docID, insertAt(1, "b"), noCursors, noCursors)
	if err != nil {
		t.Fatalf("AppendEdit 2: %v", err)
	}

	// Behavior: a fresh journal stop — the next seq, two separate undo stops.
	if seq2 != seq1+1 {
		t.Errorf("expected a fresh journal stop: seq1=%d seq2=%d, want seq2=seq1+1", seq1, seq2)
	}
	for i, want := range []string{"b", "a"} {
		step, ok := undoStep(t, s, docID)
		if !ok {
			t.Fatalf("undo %d: ok=false", i+1)
		}
		if len(step.Edits) != 1 || step.Edits[0].Insert != want {
			t.Errorf("undo %d: got %+v, want single insert %q", i+1, step.Edits, want)
		}
	}
}

func TestCoalescingWhitespaceBreaks(t *testing.T) {
	s := NewTestStore(t)
	docID := testDoc(t, s)
	advance := fixedClock(s)

	// First insert: non-whitespace.
	seqA, err := s.AppendEdit(docID, insertAt(0, "a"), noCursors, noCursors)
	if err != nil {
		t.Fatalf("AppendEdit 1: %v", err)
	}
	// Second insert: space. 'a' event's last insert is 'a' (non-whitespace),
	// so the space still coalesces INTO it — same seq.
	advance(50 * time.Millisecond)
	seqSpace, err := s.AppendEdit(docID, insertAt(1, " "), noCursors, noCursors)
	if err != nil {
		t.Fatalf("AppendEdit space: %v", err)
	}
	if seqSpace != seqA {
		t.Errorf("space must coalesce into the preceding non-whitespace stop: seqA=%d seqSpace=%d", seqA, seqSpace)
	}
	// Third insert after whitespace — the previous event now ENDS in
	// whitespace, which breaks the next coalesce: a fresh stop.
	advance(50 * time.Millisecond)
	seqB, err := s.AppendEdit(docID, insertAt(2, "b"), noCursors, noCursors)
	if err != nil {
		t.Fatalf("AppendEdit 3: %v", err)
	}
	if seqB != seqA+1 {
		t.Errorf("whitespace must break the next coalesce: seqA=%d seqB=%d, want seqB=seqA+1", seqA, seqB)
	}
}

// TestCoalescingDocBreak: coalescing is scoped to doc_id (I2: one document =
// one event stream — there is no more surface dimension to break on). Two
// single-char inserts to DIFFERENT documents within the coalesce window must
// never merge into each other's event, even though the query only orders by
// seq DESC LIMIT 1 — the WHERE doc_id=? filter is what scopes the coalescing
// candidate, and this pins that it actually does.
func TestCoalescingDocBreak(t *testing.T) {
	s := NewTestStore(t)
	docA := testDoc(t, s)
	docB := testDoc(t, s)
	advance := fixedClock(s)

	seqA, err := s.AppendEdit(docA, singleInsert("a"), noCursors, noCursors)
	if err != nil {
		t.Fatalf("AppendEdit docA: %v", err)
	}
	// Different document, well within the coalesce window — must not coalesce.
	advance(50 * time.Millisecond)
	seqB, err := s.AppendEdit(docB, singleInsert("b"), noCursors, noCursors)
	if err != nil {
		t.Fatalf("AppendEdit docB: %v", err)
	}

	// Behavior: docB's insert landed as its OWN fresh journal stop (a
	// cross-doc coalesce would have returned docA's seq), and each stream
	// undoes exactly its own insert — nothing leaked across doc_id.
	if seqB == seqA {
		t.Errorf("cross-document coalesce: docB's append returned docA's stop (seq %d)", seqA)
	}
	stepA, ok := undoStep(t, s, docA)
	if !ok || len(stepA.Edits) != 1 || stepA.Edits[0].Insert != "a" {
		t.Errorf("docA undo: got ok=%v %+v, want single insert \"a\"", ok, stepA.Edits)
	}
	stepB, ok := undoStep(t, s, docB)
	if !ok || len(stepB.Edits) != 1 || stepB.Edits[0].Insert != "b" {
		t.Errorf("docB undo: got ok=%v %+v, want single insert \"b\"", ok, stepB.Edits)
	}
}

// TestAppendEdit_NeverCoalescesIntoSnapshottedSeq and
// TestAppendEdit_TruncationInvalidatesFutureSnapshots (the WP7 regressions
// for a stale-snapshot-anchor bug the fuzz driver's new properties
// discovered) live in journal_snapshot_invalidation_test.go — extracted to
// keep this file under the 500-LoC limit (§1.6/§11).

func TestTruncateOnNewEdit(t *testing.T) {
	s3 := NewTestStore(t)
	docID := testDoc(t, s3)
	advance := fixedClock(s3)

	for _, ch := range []string{"x", "y", "z"} {
		if _, err := s3.AppendEdit(docID, singleInsert(ch), noCursors, noCursors); err != nil {
			t.Fatalf("AppendEdit %q: %v", ch, err)
		}
		advance(400 * time.Millisecond) // ensure separate stops
	}

	// Undo twice — current position moves back to seq 1.
	if _, ok := undoStep(t, s3, docID); !ok {
		t.Fatal("UndoTarget returned ok=false")
	}
	if _, ok := undoStep(t, s3, docID); !ok {
		t.Fatal("second UndoTarget returned ok=false")
	}
	cur, err := s3.CurrentSeq(docID)
	if err != nil {
		t.Fatalf("CurrentSeq: %v", err)
	}
	if cur != 1 {
		t.Fatalf("expected CurrentSeq 1 after two undos, got %d", cur)
	}

	// New edit after undo — must truncate the two abandoned future stops
	// ("y", "z"). Behavior: undoing from the new head yields "w" then "x",
	// never the truncated events.
	advance(400 * time.Millisecond)
	seqW, err := s3.AppendEdit(docID, singleInsert("w"), noCursors, noCursors)
	if err != nil {
		t.Fatalf("AppendEdit after undo: %v", err)
	}
	if curW, err := s3.CurrentSeq(docID); err != nil || curW != seqW {
		t.Fatalf("expected CurrentSeq at the new head %d, got %d (err=%v)", seqW, curW, err)
	}
	// Redo must be unavailable at the new head (the future was truncated).
	if _, ok := redoStep(t, s3, docID); ok {
		t.Error("RedoTarget should return ok=false after truncate-on-new-edit")
	}

	stepW, ok := undoStep(t, s3, docID)
	if !ok || len(stepW.Edits) != 1 || stepW.Edits[0].Insert != "w" {
		t.Fatalf("first undo after truncation: got ok=%v %+v, want insert \"w\"", ok, stepW.Edits)
	}
	stepX, ok := undoStep(t, s3, docID)
	if !ok || len(stepX.Edits) != 1 || stepX.Edits[0].Insert != "x" {
		t.Fatalf("second undo after truncation: got ok=%v %+v, want insert \"x\" (\"y\"/\"z\" must be gone)", ok, stepX.Edits)
	}
}

// TestUndoPeek_CorruptEditsSurfacesError pins §1.3: an unparseable edits
// payload must be returned as a non-nil error, never silently read as
// "nothing to undo" (the pre-v4 behavior — UndoPeek used to fold every
// failure, including a corrupt row, into ok=false).
func TestUndoPeek_CorruptEditsSurfacesError(t *testing.T) {
	s := NewTestStore(t)
	docID := testDoc(t, s)

	seedCorruptEvent(t, s, docID, `not valid json`)

	_, ok, err := s.UndoPeek(docID)
	if err == nil {
		t.Fatal("UndoPeek: want a non-nil error for a corrupt edits payload, got nil (pre-fix: silently ok=false)")
	}
	if ok {
		t.Fatal("UndoPeek: ok=true alongside a non-nil error")
	}

	_, ok, err = s.RedoPeek(docID)
	// RedoPeek: current_seq is still NULL (nothing ever committed via
	// MoveUndoPos) → "at head" → genuinely nothing to redo, not an error. The
	// corrupt row is only reachable through UndoPeek's query at this point.
	if err != nil {
		t.Fatalf("RedoPeek: unexpected error at head: %v", err)
	}
	if ok {
		t.Fatal("RedoPeek: want ok=false at head")
	}
}

// TestRedoPeek_CorruptEditsSurfacesError mirrors the UndoPeek case for the
// redo direction: a corrupt event sitting past the current undo position
// (current_seq set directly, simulating "the user undid past it") must
// surface as a RedoPeek error, never silently read as "nothing to redo".
func TestRedoPeek_CorruptEditsSurfacesError(t *testing.T) {
	s := NewTestStore(t)
	docID := testDoc(t, s)

	seedCorruptEvent(t, s, docID, `garbage`)
	if _, err := s.perm.Exec(
		`INSERT INTO session_documents(session_id, doc_id, current_seq) VALUES(?,?,0)
		 ON CONFLICT(session_id, doc_id) DO UPDATE SET current_seq=excluded.current_seq`,
		s.sessionID, docID,
	); err != nil {
		t.Fatalf("seed current_seq: %v", err)
	}

	_, ok, err := s.RedoPeek(docID)
	if err == nil {
		t.Fatal("RedoPeek: want a non-nil error for a corrupt edits payload, got nil (pre-fix: silently ok=false)")
	}
	if ok {
		t.Fatal("RedoPeek: ok=true alongside a non-nil error")
	}
}

// TestGetBlob_CorruptContentSurfacesError pins the blob-rot / bit-flip
// detection (GetBlob re-verifies SHA-256 on every read): flipping a byte in
// blobs.content must be surfaced as an error, never silently returned as if
// it were the original content.
func TestGetBlob_CorruptContentSurfacesError(t *testing.T) {
	s := NewTestStore(t)
	hash, err := s.PutBlob("original content")
	if err != nil {
		t.Fatalf("PutBlob: %v", err)
	}

	corruptBlob(t, s, hash)

	if _, err := s.GetBlob(hash); err == nil {
		t.Fatal("GetBlob: want a non-nil error for a corrupted blob, got nil (blob rot went undetected)")
	}
}

// TestCoalescingNonAdjacentBreaks pins canCoalesceInto's adjacency clause: a
// single-char insert within the 300ms window that does NOT start where the
// previous insert ended (a cursor jump) must open a fresh journal stop.
// Coalescing it would manufacture an event whose edits are sequential while
// ApplyInverse treats them as a simultaneous batch — the §1.3 overlap wedge
// canCoalesceInto exists to prevent (journal.go).
func TestCoalescingNonAdjacentBreaks(t *testing.T) {
	s := NewTestStore(t)
	docID := testDoc(t, s)
	advance := fixedClock(s)

	seq1, err := s.AppendEdit(docID, insertAt(0, "a"), noCursors, noCursors)
	if err != nil {
		t.Fatalf("AppendEdit 1: %v", err)
	}
	// 50ms later (well within the window) — but at offset 0 again, not at
	// offset 1 where the 'a' insert ended.
	advance(50 * time.Millisecond)
	seq2, err := s.AppendEdit(docID, insertAt(0, "b"), noCursors, noCursors)
	if err != nil {
		t.Fatalf("AppendEdit 2: %v", err)
	}

	if seq2 != seq1+1 {
		t.Errorf("non-adjacent insert must be a fresh journal stop: seq1=%d seq2=%d", seq1, seq2)
	}
	// Both stops undo cleanly, one edit each, newest first.
	for i, want := range []string{"b", "a"} {
		step, ok := undoStep(t, s, docID)
		if !ok {
			t.Fatalf("undo %d: ok=false", i+1)
		}
		if len(step.Edits) != 1 || step.Edits[0].Insert != want {
			t.Errorf("undo %d: got %+v, want single insert %q", i+1, step.Edits, want)
		}
	}
}

// TestCoalescingNeverIntoReplaceEvent pins canCoalesceInto's typing-run
// clause: a keystroke landing within the window right after an event whose
// last edit REPLACED text (Deleted != "" — the shape a crash-recovery or
// adopt install journals) must never coalesce into it. This is the exact
// regression behind the post-recovery ⌘Z wedge fixed in journal.go's
// canCoalesceInto: TestRecoverUnsavedEdits_AcrossStoreRestart covers it
// end-to-end but on a real clock; this is the deterministic store-level pin.
func TestCoalescingNeverIntoReplaceEvent(t *testing.T) {
	s := NewTestStore(t)
	docID := testDoc(t, s)
	advance := fixedClock(s)

	replaceAll := []buffer.AppliedEdit{{Start: 0, End: 8, Deleted: "baseline", Insert: "recovered"}}
	seq1, err := s.AppendEdit(docID, replaceAll, noCursors, noCursors)
	if err != nil {
		t.Fatalf("AppendEdit install: %v", err)
	}
	// The very next keystroke, 50ms later, typed exactly where the replace
	// ended — adjacency alone must not rescue it: the predecessor is not a
	// typing run.
	advance(50 * time.Millisecond)
	seq2, err := s.AppendEdit(docID, insertAt(len("recovered"), "x"), noCursors, noCursors)
	if err != nil {
		t.Fatalf("AppendEdit keystroke: %v", err)
	}

	if seq2 != seq1+1 {
		t.Errorf("keystroke after a replace event must be a fresh journal stop: seq1=%d seq2=%d", seq1, seq2)
	}
	// Undo the keystroke alone, then the install alone — no overlap error.
	step, ok := undoStep(t, s, docID)
	if !ok {
		t.Fatal("undo 1: ok=false")
	}
	if len(step.Edits) != 1 || step.Edits[0].Insert != "x" {
		t.Errorf("undo 1: got %+v, want the single 'x' keystroke", step.Edits)
	}
	step, ok = undoStep(t, s, docID)
	if !ok {
		t.Fatal("undo 2: ok=false")
	}
	if len(step.Edits) != 1 || step.Edits[0].Deleted != "baseline" {
		t.Errorf("undo 2: got %+v, want the install replace", step.Edits)
	}
}
