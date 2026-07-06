package docstate

import (
	"strings"
	"testing"

	"rune/pkg/editor/buffer"
	"rune/pkg/editor/cursor"
	"rune/pkg/vfs"
)

// Review finding 3: the undo-unwind Diverged override must be an EXISTS over
// ALL correlated observations, never a property of whichever observation
// happens to be NEWEST — a bare seq-NULL sighting (flush-tick probe) recorded
// after an adoption otherwise permanently defeats the override, letting a
// wound-back save pass CAS and clobber the adopted external bytes.
// Pre-fix: Sync reported SyncDiskAhead here (not even dirty); the fix makes
// the interposed sighting irrelevant.
func TestSync_UnwindOverrideSurvivesBareSighting(t *testing.T) {
	s, mem := newMatTestStore(t)
	const path = "/note.md"
	const original = "original\n"
	if err := mem.WriteFile(path, []byte(original), 0o644); err != nil {
		t.Fatalf("seed: %v", err)
	}

	loaded, err := s.Load(path)
	if err != nil {
		t.Fatalf("Load: %v", err)
	}
	docID := loaded.DocID

	// One real edit, then save it: the save observation is the adoption whose
	// seq the undo below unwinds past.
	seq, err := s.AppendEdit(docID,
		[]buffer.AppliedEdit{{Start: 0, End: 0, Insert: "x", Deleted: ""}},
		[]cursor.Cursor{{Position: 0}}, []cursor.Cursor{{Position: 1}})
	if err != nil {
		t.Fatalf("AppendEdit: %v", err)
	}
	expect, ok, err := s.SavedObs(docID)
	if err != nil || !ok {
		t.Fatalf("SavedObs: ok=%v err=%v", ok, err)
	}
	res, err := s.Materialize(docID, path, "x"+original, expect.ID, seq, false)
	if err != nil || !res.Committed {
		t.Fatalf("Materialize: committed=%v err=%v", res.Committed, err)
	}

	// Undo below the save's adoption seq.
	if err := s.MoveUndoPos(docID, 0); err != nil {
		t.Fatalf("MoveUndoPos: %v", err)
	}

	// A bare flush-tick probe records a seq-NULL sighting that becomes the
	// NEWEST observation (disk still holds the saved bytes; the wound-back
	// reconstruction differs, so no auto-adopt fires).
	if _, err := s.Probe(docID); err != nil {
		t.Fatalf("Probe: %v", err)
	}

	sync, err := s.Sync(docID)
	if err != nil {
		t.Fatalf("Sync: %v", err)
	}
	if sync.Kind != SyncDiverged {
		t.Fatalf("Sync after undo-below-adoption with interposed bare sighting = %v, want SyncDiverged (the override must not depend on the newest observation being the adoption)", sync.Kind)
	}
	dirty, err := s.IsDirty(docID)
	if err != nil {
		t.Fatalf("IsDirty: %v", err)
	}
	if !dirty {
		t.Fatal("doc must read dirty while unwound below an adoption — a clean read lets quit skip the guard entirely")
	}
}

// Review finding 6: ResolveAbandon unwinds RESOLUTIONS only. When the merge
// entry's ResolveAdopt failed (baseline still the last genuine load/save
// agreement), an Esc-abort must refuse — pre-fix it deleted that genuine
// observation row and regressed the CAS baseline to its supersedes.
func TestResolveAbandon_RefusesNonResolveBaseline(t *testing.T) {
	s, mem := newMatTestStore(t)
	const path = "/note.md"
	if err := mem.WriteFile(path, []byte("content\n"), 0o644); err != nil {
		t.Fatalf("seed: %v", err)
	}
	loaded, err := s.Load(path)
	if err != nil {
		t.Fatalf("Load: %v", err)
	}
	docID := loaded.DocID

	before, ok, err := s.SavedObs(docID)
	if err != nil || !ok {
		t.Fatalf("SavedObs: ok=%v err=%v", ok, err)
	}
	if before.Origin == "resolve" {
		t.Fatalf("test precondition: first-load baseline origin = %q, want a non-resolve origin", before.Origin)
	}

	abandonErr := s.ResolveAbandon(docID)
	if abandonErr == nil {
		t.Fatal("ResolveAbandon on a non-resolve baseline must refuse — deleting a genuine load/save agreement destroys observation history")
	}
	if !strings.Contains(abandonErr.Error(), "refusing") {
		t.Fatalf("ResolveAbandon error should explain the refusal, got: %v", abandonErr)
	}

	after, ok, err := s.SavedObs(docID)
	if err != nil || !ok {
		t.Fatalf("SavedObs after refused abandon: ok=%v err=%v", ok, err)
	}
	if after.ID != before.ID {
		t.Fatalf("saved_obs moved across a REFUSED abandon: %d -> %d", before.ID, after.ID)
	}
}

// Keep the vfs import anchored even if the helpers above change shape.
var _ vfs.FS = (*vfs.Mem)(nil)
