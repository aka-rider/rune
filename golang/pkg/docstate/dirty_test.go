package docstate

import (
	"database/sql"
	"testing"
	"time"

	"rune/pkg/editor/buffer"
)

// markSavedNow simulates the disk-write side of a save WITHOUT touching disk
// (this file's tests never wire a vfs.FS): records a 'save'-origin
// observation of the doc's CURRENT reconstructed content, correlated to the
// current journal position, and advances saved_obs to it — the same two
// facts Materialize's commitSave commits in its one tx, minus the actual
// disk I/O. A white-box helper (same package) standing in for the pre-v4
// MarkSavedAt this test used to drive directly.
func markSavedNow(t *testing.T, s *Store, docID int64) {
	t.Helper()
	content, err := s.RecoverDocument(docID)
	if err != nil {
		t.Fatalf("markSavedNow: RecoverDocument: %v", err)
	}
	hash, err := s.PutBlob(content)
	if err != nil {
		t.Fatalf("markSavedNow: PutBlob: %v", err)
	}
	seq, err := s.CurrentSeq(docID)
	if err != nil {
		t.Fatalf("markSavedNow: CurrentSeq: %v", err)
	}
	at := s.clock().UTC().Format(time.RFC3339Nano)
	obsID, err := s.recordObservation(docID, hash, sql.NullInt64{Int64: seq, Valid: true}, 0, "", sql.NullInt64{}, sql.NullInt64{}, sql.NullInt64{}, "save", at)
	if err != nil {
		t.Fatalf("markSavedNow: recordObservation: %v", err)
	}
	if err := s.setSavedObs(docID, obsID); err != nil {
		t.Fatalf("markSavedNow: setSavedObs: %v", err)
	}
}

// newDirtyTestStore creates an in-memory Store for dirty-tracking tests,
// with its own established session (v10).
func newDirtyTestStore(t *testing.T) *Store {
	t.Helper()
	perm, err := openPerm(":memory:")
	if err != nil {
		t.Fatalf("openPerm: %v", err)
	}
	sessionID, err := establishSession(perm, time.Now)
	if err != nil {
		t.Fatalf("establish session: %v", err)
	}
	s := &Store{perm: perm, clock: time.Now, sessionID: sessionID, livenessCheck: isProcessAlive}
	t.Cleanup(func() { perm.Close() })
	return s
}

// wordInsert creates a multi-char (non-coalesceable) edit. Single-char inserts
// within 300ms coalesce, making undo a no-op on the second step. Use this for
// tests that need discrete undo steps or accurate post-save dirty detection.
func wordInsert(text string) []buffer.AppliedEdit {
	return []buffer.AppliedEdit{{Start: 0, End: 0, Deleted: "", Insert: text}}
}

// appendWord appends a non-coalesceable edit for docID.
func appendWord(s *Store, docID int64, text string) (int64, error) {
	return s.AppendEdit(docID, wordInsert(text), noCursors, noCursors)
}

// TestIsDirty_Pristine: a doc with no events is clean.
func TestIsDirty_Pristine(t *testing.T) {
	s := newDirtyTestStore(t)
	ref, err := s.CreateScratch("pristine")
	if err != nil {
		t.Fatalf("create: %v", err)
	}
	dirty, err := s.IsDirty(ref.ID)
	if err != nil {
		t.Fatalf("IsDirty: %v", err)
	}
	if dirty {
		t.Fatal("want clean, got dirty for doc with no events")
	}
}

// TestIsDirty_AfterEdit: a doc becomes dirty after an edit.
func TestIsDirty_AfterEdit(t *testing.T) {
	s := newDirtyTestStore(t)
	ref, err := s.CreateScratch("edit-doc")
	if err != nil {
		t.Fatalf("create: %v", err)
	}
	if _, err := s.AppendEdit(ref.ID, singleInsert("A"), noCursors, noCursors); err != nil {
		t.Fatalf("AppendEdit: %v", err)
	}
	dirty, err := s.IsDirty(ref.ID)
	if err != nil {
		t.Fatalf("IsDirty: %v", err)
	}
	if !dirty {
		t.Fatal("want dirty after edit, got clean")
	}
}

// TestIsDirty_GlobalSeqRegression is the core regression:
// open a throwaway doc first to advance the global AUTOINCREMENT counter
// so the target doc's first event seq > 1. After edit×2 + undo×2 the
// undo pointer lands at firstEventSeq-1. The old effectiveJournalPos
// compared against 0 and was permanently dirty; IsDirty must correctly
// report clean.
func TestIsDirty_GlobalSeqRegression(t *testing.T) {
	s := newDirtyTestStore(t)

	// Advance the global seq counter via a throwaway doc (wordInsert = no coalesce).
	throwaway, err := s.CreateScratch("throwaway")
	if err != nil {
		t.Fatalf("create throwaway: %v", err)
	}
	if _, err := appendWord(s, throwaway.ID, "seed"); err != nil {
		t.Fatalf("throwaway edit: %v", err)
	}

	// Now create the target doc.
	ref, err := s.CreateScratch("target")
	if err != nil {
		t.Fatalf("create target: %v", err)
	}
	docID := ref.ID

	// edit × 2 (non-coalesceable → 2 separate undo stops)
	if _, err := appendWord(s, docID, "edit-one"); err != nil {
		t.Fatalf("edit 1: %v", err)
	}
	if _, err := appendWord(s, docID, "edit-two"); err != nil {
		t.Fatalf("edit 2: %v", err)
	}

	// undo × 2 — lands at firstEventSeq-1 (below the doc's first event)
	for i := range 2 {
		_, ok := undoStep(t, s, docID)
		if !ok {
			t.Fatalf("undo %d: no undo target", i+1)
		}
	}

	// Must be clean: all edits have been undone.
	dirty, err := s.IsDirty(docID)
	if err != nil {
		t.Fatalf("IsDirty: %v", err)
	}
	if dirty {
		t.Fatal("want clean after undo-all, got dirty (global-seq regression)")
	}
}

// TestIsDirty_AncestorDerived_SaveUndoRedo: dirty → save → clean; edit after
// → dirty; undo back → clean; redo past → dirty. Ancestor-derived (WP5): a
// "save" is now recorded as a saved_obs observation (markSavedNow stands in
// for Materialize's disk write), and IsDirty is Sync(docID).Kind != Clean —
// there is no separate saved-position column anymore.
func TestIsDirty_AncestorDerived_SaveUndoRedo(t *testing.T) {
	s := newDirtyTestStore(t)
	ref, err := s.CreateScratch("save-doc")
	if err != nil {
		t.Fatalf("create: %v", err)
	}
	docID := ref.ID

	// Use wordInsert (multi-char) so the post-save edit can't coalesce with the
	// pre-save edit. Single-char inserts within 300ms merge into one event,
	// which would make IsDirty incorrectly report clean after coalescing.
	if _, err := appendWord(s, docID, "before-save"); err != nil {
		t.Fatalf("edit: %v", err)
	}

	// dirty before save
	if dirty, err := s.IsDirty(docID); err != nil || !dirty {
		t.Fatalf("want dirty before save, got dirty=%v err=%v", dirty, err)
	}

	markSavedNow(t, s, docID)

	// clean after save
	if dirty, err := s.IsDirty(docID); err != nil || dirty {
		t.Fatalf("want clean after save, got dirty=%v err=%v", dirty, err)
	}

	// edit after save → dirty (wordInsert prevents coalescing with pre-save event)
	if _, err := appendWord(s, docID, "after-save"); err != nil {
		t.Fatalf("post-save edit: %v", err)
	}
	if dirty, err := s.IsDirty(docID); err != nil || !dirty {
		t.Fatalf("want dirty after post-save edit, got dirty=%v err=%v", dirty, err)
	}

	// undo back to save point → clean
	if _, ok := undoStep(t, s, docID); !ok {
		t.Fatal("undo: no target")
	}
	if dirty, err := s.IsDirty(docID); err != nil || dirty {
		t.Fatalf("want clean at save point after undo, got dirty=%v err=%v", dirty, err)
	}

	// redo past save point → dirty
	if _, ok := redoStep(t, s, docID); !ok {
		t.Fatal("redo: no target")
	}
	if dirty, err := s.IsDirty(docID); err != nil || !dirty {
		t.Fatalf("want dirty past save point after redo, got dirty=%v err=%v", dirty, err)
	}
}

// TestCurrentSeq: reports 0 for no events, head seq after edits, mid-undo seq after undo.
func TestCurrentSeq(t *testing.T) {
	s := newDirtyTestStore(t)
	ref, err := s.CreateScratch("seq-doc")
	if err != nil {
		t.Fatalf("create: %v", err)
	}
	docID := ref.ID

	// No events → 0
	seq, err := s.CurrentSeq(docID)
	if err != nil || seq != 0 {
		t.Fatalf("want seq=0 for fresh doc, got %d err=%v", seq, err)
	}

	// After two edits → head seq (wordInsert avoids coalescing into one event).
	s1, err := appendWord(s, docID, "first")
	if err != nil {
		t.Fatalf("edit 1: %v", err)
	}
	s2, err := appendWord(s, docID, "second")
	if err != nil {
		t.Fatalf("edit 2: %v", err)
	}
	seq, err = s.CurrentSeq(docID)
	if err != nil || seq != s2 {
		t.Fatalf("want seq=%d (head), got %d err=%v", s2, seq, err)
	}

	// After undo → mid-undo seq
	step, ok := undoStep(t, s, docID)
	if !ok {
		t.Fatal("undo: no target")
	}
	seq, err = s.CurrentSeq(docID)
	if err != nil || seq != step.NewPos {
		t.Fatalf("want seq=%d (mid-undo), got %d err=%v", step.NewPos, seq, err)
	}
	_ = s1
}
