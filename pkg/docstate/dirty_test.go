package docstate

import (
	"testing"
	"time"

	"rune/pkg/editor/buffer"
)

// newDirtyTestStore creates an in-memory Store for dirty-tracking tests.
func newDirtyTestStore(t *testing.T) *Store {
	t.Helper()
	perm, err := openPerm(":memory:")
	if err != nil {
		t.Fatalf("openPerm: %v", err)
	}
	s := &Store{perm: perm, clock: time.Now}
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
	return s.AppendEdit(docID, "main", wordInsert(text), noCursors, noCursors, "main")
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
	if _, err := s.AppendEdit(ref.ID, "main", singleInsert("A"), noCursors, noCursors, "main"); err != nil {
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
	for i := 0; i < 2; i++ {
		_, _, _, _, ok := undoStep(t, s, docID)
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

// TestMarkSaved: dirty → MarkSaved → clean; edit after → dirty; undo back → clean; redo past → dirty.
func TestMarkSaved(t *testing.T) {
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

	// Capture the position synchronously and stamp it (the production save path):
	// MarkSavedAt records the saved position, never the live head (§1.4.2).
	savedSeq, err := s.CurrentSeq(docID)
	if err != nil {
		t.Fatalf("CurrentSeq: %v", err)
	}
	if err := s.MarkSavedAt(docID, savedSeq); err != nil {
		t.Fatalf("MarkSavedAt: %v", err)
	}

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
	if _, _, _, _, ok := undoStep(t, s, docID); !ok {
		t.Fatal("undo: no target")
	}
	if dirty, err := s.IsDirty(docID); err != nil || dirty {
		t.Fatalf("want clean at save point after undo, got dirty=%v err=%v", dirty, err)
	}

	// redo past save point → dirty
	if _, _, _, _, ok := redoStep(t, s, docID); !ok {
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
	_, _, _, midSeq, ok := undoStep(t, s, docID)
	if !ok {
		t.Fatal("undo: no target")
	}
	seq, err = s.CurrentSeq(docID)
	if err != nil || seq != midSeq {
		t.Fatalf("want seq=%d (mid-undo), got %d err=%v", midSeq, seq, err)
	}
	_ = s1
}
