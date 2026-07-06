package docstate

import (
	"database/sql"
	"testing"
)

// TestAncestorAt_IDTiebreakOverSeq pins the same invariant the pre-v4
// DiskBaselineContent write-recency tie-break protected (§B2 / plan
// "position-derived ancestor"): ancestorAt orders candidates by seq DESC,
// id DESC — two observations correlated to the SAME seq (a save recorded
// after an undo re-lands at an earlier seq than a prior save) must resolve
// to the most-recently-INSERTED one (id DESC), never an arbitrary pick.
func TestAncestorAt_IDTiebreakOverSeq(t *testing.T) {
	s := NewTestStore(t)
	docID := testDoc(t, s)

	const older = "v1 — saved at seq 5"
	const newer = "v2 — saved at seq 5 (inserted later)"
	oldHash, err := s.PutBlob(older)
	if err != nil {
		t.Fatalf("PutBlob older: %v", err)
	}
	newHash, err := s.PutBlob(newer)
	if err != nil {
		t.Fatalf("PutBlob newer: %v", err)
	}
	if _, err := s.recordObservation(docID, oldHash, sql.NullInt64{Int64: 5, Valid: true}, 0, "", sql.NullInt64{}, sql.NullInt64{}, sql.NullInt64{}, "save", ""); err != nil {
		t.Fatalf("recordObservation older: %v", err)
	}
	if _, err := s.recordObservation(docID, newHash, sql.NullInt64{Int64: 5, Valid: true}, 0, "", sql.NullInt64{}, sql.NullInt64{}, sql.NullInt64{}, "save", ""); err != nil {
		t.Fatalf("recordObservation newer: %v", err)
	}

	anc, found, err := s.ancestorAt(docID, 5, 0)
	if err != nil {
		t.Fatalf("ancestorAt: %v", err)
	}
	if !found {
		t.Fatal("ancestorAt: expected an ancestor")
	}
	if anc.BlobHash != newHash {
		t.Fatalf("ancestorAt tie-break: got hash of %q, want the id-newest %q", "?", newer)
	}
}

// TestAncestorAt_IsolatedPerDoc: an ancestor observation for one document
// must never leak into another document's ancestorAt result.
func TestAncestorAt_IsolatedPerDoc(t *testing.T) {
	s := NewTestStore(t)
	docA := testDoc(t, s)
	docB := testDoc(t, s)

	hashA, err := s.PutBlob("doc A content")
	if err != nil {
		t.Fatalf("PutBlob: %v", err)
	}
	if _, err := s.recordObservation(docA, hashA, sql.NullInt64{Int64: 0, Valid: true}, 0, "", sql.NullInt64{}, sql.NullInt64{}, sql.NullInt64{}, "load", ""); err != nil {
		t.Fatalf("recordObservation: %v", err)
	}

	if _, found, err := s.ancestorAt(docB, 0, 0); err != nil {
		t.Fatalf("ancestorAt docB: %v", err)
	} else if found {
		t.Fatal("ancestorAt: doc B must not see doc A's ancestor observation")
	}

	anc, found, err := s.ancestorAt(docA, 0, 0)
	if err != nil {
		t.Fatalf("ancestorAt docA: %v", err)
	}
	if !found || anc.BlobHash != hashA {
		t.Fatalf("ancestorAt docA: found=%v hash=%q, want %q", found, anc.BlobHash, hashA)
	}
}

// TestAncestorAt_IgnoresUncorrelatedProbes: a 'probe' observation (seq=NULL,
// always uncorrelated) must never be picked as an ancestor, even when it is
// the most recent observation by id.
func TestAncestorAt_IgnoresUncorrelatedProbes(t *testing.T) {
	s := NewTestStore(t)
	docID := testDoc(t, s)

	loadHash, err := s.PutBlob("loaded content")
	if err != nil {
		t.Fatalf("PutBlob load: %v", err)
	}
	if _, err := s.recordObservation(docID, loadHash, sql.NullInt64{Int64: 0, Valid: true}, 0, "", sql.NullInt64{}, sql.NullInt64{}, sql.NullInt64{}, "load", ""); err != nil {
		t.Fatalf("recordObservation load: %v", err)
	}
	probeHash, err := s.PutBlob("probed content, never correlated")
	if err != nil {
		t.Fatalf("PutBlob probe: %v", err)
	}
	if _, err := s.recordObservation(docID, probeHash, sql.NullInt64{}, 0, "", sql.NullInt64{}, sql.NullInt64{}, sql.NullInt64{}, "probe", ""); err != nil {
		t.Fatalf("recordObservation probe: %v", err)
	}

	anc, found, err := s.ancestorAt(docID, 0, 0)
	if err != nil {
		t.Fatalf("ancestorAt: %v", err)
	}
	if !found {
		t.Fatal("ancestorAt: expected the load observation to be found")
	}
	if anc.BlobHash != loadHash {
		t.Fatalf("ancestorAt: got %q, want the load observation %q (probe must never win)", anc.BlobHash, loadHash)
	}
}
