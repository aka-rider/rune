package docstate

import (
	"testing"

	"rune/pkg/editor/buffer"
)

// TestLoad_FreshFile_NoHistoryIsClean: loading a file with no prior journal
// history reports HasHistory=false, Recovered==DiskContent, and a Clean sync
// (ours reconstructs to exactly what we just loaded, theirs is the load
// observation we just recorded — trivially equal).
func TestLoad_FreshFile_NoHistoryIsClean(t *testing.T) {
	s, mem := newMatTestStore(t)
	const path = "/note.md"
	if err := mem.WriteFile(path, []byte("hello"), 0o644); err != nil {
		t.Fatalf("seed file: %v", err)
	}

	result, err := s.Load(path)
	if err != nil {
		t.Fatalf("Load: %v", err)
	}
	if result.HasHistory {
		t.Fatal("fresh file: HasHistory should be false")
	}
	if result.DiskContent != "hello" || result.Recovered != "hello" {
		t.Fatalf("DiskContent=%q Recovered=%q, want both %q", result.DiskContent, result.Recovered, "hello")
	}
	if result.Sync.Kind != SyncClean {
		t.Fatalf("Sync.Kind = %v, want SyncClean", result.Sync.Kind)
	}

	// The load observation is recorded and becomes the CAS baseline.
	saved, hasSaved, err := s.SavedObs(result.DocID)
	if err != nil || !hasSaved {
		t.Fatalf("SavedObs: hasSaved=%v err=%v", hasSaved, err)
	}
	if saved.Origin != "load" {
		t.Fatalf("saved_obs.Origin = %q, want %q", saved.Origin, "load")
	}
	if saved.BlobHash != hashBytes([]byte("hello")) {
		t.Fatalf("saved_obs.BlobHash = %q, want hash of %q", saved.BlobHash, "hello")
	}

	// It is also the ancestor at position 0.
	anc, hasAnc, err := s.ancestorAt(result.DocID, 0, 0)
	if err != nil || !hasAnc {
		t.Fatalf("ancestorAt(pos=0): hasAnc=%v err=%v", hasAnc, err)
	}
	if anc.ID != saved.ID {
		t.Fatalf("ancestorAt(pos=0) = observation %d, want the load observation %d", anc.ID, saved.ID)
	}
}

// TestLoad_ReopenWithJournalHistory_ReconstructsFromJournal: a document with
// prior journal history reconstructs Recovered from the journal, which may
// differ from the raw disk bytes (unsaved edits recorded but never
// materialized).
func TestLoad_ReopenWithJournalHistory_ReconstructsFromJournal(t *testing.T) {
	s, mem := newMatTestStore(t)
	const path = "/note.md"
	if err := mem.WriteFile(path, []byte("saved content"), 0o644); err != nil {
		t.Fatalf("seed file: %v", err)
	}

	first, err := s.Load(path)
	if err != nil {
		t.Fatalf("first Load: %v", err)
	}
	if _, err := s.AppendEdit(first.DocID, singleInsert("X"), noCursors, noCursors); err != nil {
		t.Fatalf("AppendEdit: %v", err)
	}
	// Note: the edit is journaled but never materialized to disk — disk
	// still says "saved content".

	second, err := s.Load(path)
	if err != nil {
		t.Fatalf("second Load: %v", err)
	}
	if second.DocID != first.DocID {
		t.Fatalf("second Load resolved a different docID: %d != %d", second.DocID, first.DocID)
	}
	if !second.HasHistory {
		t.Fatal("second Load: HasHistory should be true (an edit was journaled)")
	}
	if second.DiskContent != "saved content" {
		t.Fatalf("DiskContent = %q, want the raw disk bytes %q", second.DiskContent, "saved content")
	}
	if second.Recovered != "Xsaved content" {
		t.Fatalf("Recovered = %q, want the journal reconstruction %q", second.Recovered, "Xsaved content")
	}
}

// TestLoad_ReopenWithLocalEditAndExternalChange_Diverges is the genuine
// three-way conflict Load must detect: a local, journaled-but-unsaved edit
// AND an independent external disk change since the first Load. Regression
// test for a structural bug found during WP5 integration: Load used to
// compute Sync AFTER recording its own fresh observation, correlated to the
// SAME journal position ancestorAt(docID, pos) searches at — so the fresh
// disk reading was picked up as its OWN ancestor (self-comparison), making
// `theirs.Hash == ancestor.Hash` trivially true and misclassifying a real
// Diverged conflict as an ordinary BufferAhead unsaved edit, silently
// dropping the reload-time conflict guard this exact scenario exists to
// raise (§1.4.7 / plan R1-R3, the Conflict lifecycle).
func TestLoad_ReopenWithLocalEditAndExternalChange_Diverges(t *testing.T) {
	s, mem := newMatTestStore(t)
	const path = "/note.md"
	const ancestor = "shared\noriginal\n"
	const ours = "shared\nours changed\n"
	const theirs = "shared\ntheirs changed\n"

	if err := mem.WriteFile(path, []byte(ancestor), 0o644); err != nil {
		t.Fatalf("seed file: %v", err)
	}
	first, err := s.Load(path)
	if err != nil {
		t.Fatalf("first Load: %v", err)
	}
	if first.Sync.Kind != SyncClean {
		t.Fatalf("first Load: Sync.Kind = %v, want SyncClean", first.Sync.Kind)
	}

	// A local, journaled edit diverges ours from the ancestor. Deleted must
	// be set for buffer.ReplayForward to treat this as a REPLACEMENT (it
	// skips len(Deleted) bytes at Start, not End-Start) — omitting it would
	// replay as a pure insert, leaving the ancestor's tail concatenated on.
	if _, err := s.AppendEdit(first.DocID,
		[]buffer.AppliedEdit{{Start: 0, End: len(ancestor), Deleted: ancestor, Insert: ours}}, nil, nil); err != nil {
		t.Fatalf("AppendEdit: %v", err)
	}

	// An INDEPENDENT external change lands on disk — never the journaled ours.
	if err := mem.WriteFile(path, []byte(theirs), 0o644); err != nil {
		t.Fatalf("write external change: %v", err)
	}

	second, err := s.Load(path)
	if err != nil {
		t.Fatalf("second Load: %v", err)
	}
	if second.DocID != first.DocID {
		t.Fatalf("second Load resolved a different docID: %d != %d", second.DocID, first.DocID)
	}
	if second.Recovered != ours {
		t.Fatalf("Recovered = %q, want the journal reconstruction %q", second.Recovered, ours)
	}
	if second.DiskContent != theirs {
		t.Fatalf("DiskContent = %q, want the fresh external bytes %q", second.DiskContent, theirs)
	}
	if second.Sync.Kind != SyncDiverged {
		t.Fatalf("second Load: Sync.Kind = %v, want SyncDiverged (local edit + external change both diverged from the ancestor)", second.Sync.Kind)
	}
	if second.Sync.Ancestor.Hash != hashBytes([]byte(ancestor)) {
		t.Fatalf("Sync.Ancestor hash does not match the FIRST load's observation (%q) — self-referenced its own fresh reading instead", ancestor)
	}
}
