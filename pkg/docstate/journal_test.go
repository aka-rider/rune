package docstate

import (
	"testing"
	"time"

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

	now := time.Now()
	s.clock = func() time.Time { return now }

	// Event A.
	if _, err := s.AppendEdit(docID, singleInsert("a"), noCursors, noCursors); err != nil {
		t.Fatalf("AppendEdit A: %v", err)
	}
	// Event B (outside 300ms window → separate undo-stop).
	now = now.Add(400 * time.Millisecond)
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

	// Fix the clock at t=0.
	now := time.Now()
	s.clock = func() time.Time { return now }

	if _, err := s.AppendEdit(docID, singleInsert("a"), noCursors, noCursors); err != nil {
		t.Fatalf("AppendEdit 1: %v", err)
	}
	// Advance clock by 100ms (within 300ms window).
	now = now.Add(100 * time.Millisecond)
	if _, err := s.AppendEdit(docID, singleInsert("b"), noCursors, noCursors); err != nil {
		t.Fatalf("AppendEdit 2: %v", err)
	}

	n := countRows(t, s.perm, `SELECT COUNT(*) FROM events WHERE doc_id=?`, docID)
	if n != 1 {
		t.Errorf("expected 1 coalesced event row, got %d", n)
	}
}

func TestCoalescingOutsideWindow(t *testing.T) {
	s := NewTestStore(t)
	docID := testDoc(t, s)

	now := time.Now()
	s.clock = func() time.Time { return now }

	if _, err := s.AppendEdit(docID, singleInsert("a"), noCursors, noCursors); err != nil {
		t.Fatalf("AppendEdit 1: %v", err)
	}
	// Advance clock by 400ms (outside 300ms window).
	now = now.Add(400 * time.Millisecond)
	if _, err := s.AppendEdit(docID, singleInsert("b"), noCursors, noCursors); err != nil {
		t.Fatalf("AppendEdit 2: %v", err)
	}

	n := countRows(t, s.perm, `SELECT COUNT(*) FROM events WHERE doc_id=?`, docID)
	if n != 2 {
		t.Errorf("expected 2 event rows (no coalesce), got %d", n)
	}
}

func TestCoalescingWhitespaceBreaks(t *testing.T) {
	s := NewTestStore(t)
	docID := testDoc(t, s)

	now := time.Now()
	s.clock = func() time.Time { return now }

	// First insert: non-whitespace.
	if _, err := s.AppendEdit(docID, singleInsert("a"), noCursors, noCursors); err != nil {
		t.Fatalf("AppendEdit 1: %v", err)
	}
	// Second insert: space — must break coalesce.
	now = now.Add(50 * time.Millisecond)
	if _, err := s.AppendEdit(docID, singleInsert(" "), noCursors, noCursors); err != nil {
		t.Fatalf("AppendEdit space: %v", err)
	}
	// Third insert after whitespace — new stop.
	now = now.Add(50 * time.Millisecond)
	if _, err := s.AppendEdit(docID, singleInsert("b"), noCursors, noCursors); err != nil {
		t.Fatalf("AppendEdit 3: %v", err)
	}

	n := countRows(t, s.perm, `SELECT COUNT(*) FROM events WHERE doc_id=?`, docID)
	// Sequence: [a] + [space] (space coalesces into 'a' event? No — space is inserted
	// AFTER 'a', so 'a' event's last insert is 'a' (non-whitespace), space coalesces into it.
	// Then 'b' tries to coalesce: last event's last insert is ' ' (whitespace) → no coalesce.
	// Result: 2 events: [a+space] and [b].
	if n != 2 {
		t.Errorf("expected 2 events (whitespace breaks next coalesce), got %d", n)
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

	now := time.Now()
	s.clock = func() time.Time { return now }

	if _, err := s.AppendEdit(docA, singleInsert("a"), noCursors, noCursors); err != nil {
		t.Fatalf("AppendEdit docA: %v", err)
	}
	// Different document, well within the coalesce window — must not coalesce.
	now = now.Add(50 * time.Millisecond)
	if _, err := s.AppendEdit(docB, singleInsert("b"), noCursors, noCursors); err != nil {
		t.Fatalf("AppendEdit docB: %v", err)
	}

	if n := countRows(t, s.perm, `SELECT COUNT(*) FROM events WHERE doc_id=?`, docA); n != 1 {
		t.Errorf("docA: expected 1 event, got %d", n)
	}
	if n := countRows(t, s.perm, `SELECT COUNT(*) FROM events WHERE doc_id=?`, docB); n != 1 {
		t.Errorf("docB: expected 1 event, got %d", n)
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

	nowT := time.Now()
	s3.clock = func() time.Time { return nowT }

	for _, ch := range []string{"x", "y", "z"} {
		if _, err := s3.AppendEdit(docID, singleInsert(ch), noCursors, noCursors); err != nil {
			t.Fatalf("AppendEdit %q: %v", ch, err)
		}
		nowT = nowT.Add(400 * time.Millisecond) // ensure separate stops
	}

	before := countRows(t, s3.perm, `SELECT COUNT(*) FROM events WHERE doc_id=?`, docID)
	if before != 3 {
		t.Fatalf("expected 3 events before undo, got %d", before)
	}

	// Undo once to step back.
	if _, ok := undoStep(t, s3, docID); !ok {
		t.Fatal("UndoTarget returned ok=false")
	}
	// Undo again.
	if _, ok := undoStep(t, s3, docID); !ok {
		t.Fatal("second UndoTarget returned ok=false")
	}

	// New edit after undo — must truncate the two future events.
	nowT = nowT.Add(400 * time.Millisecond)
	if _, err := s3.AppendEdit(docID, singleInsert("w"), noCursors, noCursors); err != nil {
		t.Fatalf("AppendEdit after undo: %v", err)
	}

	after := countRows(t, s3.perm, `SELECT COUNT(*) FROM events WHERE doc_id=?`, docID)
	// Before undo we were at seq 3. After two undos current_seq = 1 (at or before seq 1).
	// New edit truncates seq > current_seq (seq 2 and 3), then inserts new event.
	// Remaining: seq 1 + new event = 2 events.
	if after != 2 {
		t.Errorf("expected 2 events after truncate-on-new-edit, got %d", after)
	}

	// Redo should now return false (future was truncated).
	_, ok := redoStep(t, s3, docID)
	if ok {
		t.Error("RedoTarget should return ok=false after truncate-on-new-edit")
	}
}

// TestUndoPeek_CorruptEditsSurfacesError pins §1.3: an unparseable edits
// payload must be returned as a non-nil error, never silently read as
// "nothing to undo" (the pre-v4 behavior — UndoPeek used to fold every
// failure, including a corrupt row, into ok=false).
func TestUndoPeek_CorruptEditsSurfacesError(t *testing.T) {
	s := NewTestStore(t)
	docID := testDoc(t, s)

	now := "2026-01-01T00:00:00Z"
	if _, err := s.perm.Exec(
		`INSERT INTO events(doc_id, session_id, edits, cursors_before, cursors_after, at) VALUES(?,?,?,?,?,?)`,
		docID, s.sessionID, `not valid json`, `[]`, `[]`, now,
	); err != nil {
		t.Fatalf("seed corrupt event: %v", err)
	}

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

	if _, err := s.perm.Exec(
		`INSERT INTO events(doc_id, session_id, edits, cursors_before, cursors_after, at) VALUES(?,?,?,?,?,?)`,
		docID, s.sessionID, `garbage`, `[]`, `[]`, "2026-01-01T00:00:00Z",
	); err != nil {
		t.Fatalf("seed corrupt event: %v", err)
	}
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

	var compressed []byte
	if err := s.perm.QueryRow(`SELECT content FROM blobs WHERE hash=?`, hash).Scan(&compressed); err != nil {
		t.Fatalf("read compressed blob: %v", err)
	}
	if len(compressed) == 0 {
		t.Fatal("precondition: expected non-empty compressed blob")
	}
	corrupted := append([]byte(nil), compressed...)
	corrupted[len(corrupted)-1] ^= 0xFF // flip a byte

	if _, err := s.perm.Exec(`UPDATE blobs SET content=? WHERE hash=?`, corrupted, hash); err != nil {
		t.Fatalf("corrupt blob: %v", err)
	}

	if _, err := s.GetBlob(hash); err == nil {
		t.Fatal("GetBlob: want a non-nil error for a corrupted blob, got nil (blob rot went undetected)")
	}
}
