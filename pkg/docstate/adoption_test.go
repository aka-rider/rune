package docstate

import (
	"testing"

	"rune/pkg/editor/buffer"
)

// TestSync_TheirsIsNewestSighting is the B1 regression: Sync/syncWithTheirs
// must classify against the doc's NEWEST recorded observation (any origin),
// never against saved_obs — a bare sighting (Probe, no adoption) must still
// be visible to the conflict lifecycle even though it never moved the CAS
// baseline. Pre-fix, Sync read SavedObs as theirs, so a bare Probe's fresh
// sighting was invisible to Sync entirely (Sync would trivially read
// ours==theirs==the STALE saved_obs as Clean).
func TestSync_TheirsIsNewestSighting(t *testing.T) {
	s, mem := newMatTestStore(t)
	const path = "/note.md"
	if err := mem.WriteFile(path, []byte("agreed"), 0o644); err != nil {
		t.Fatalf("seed file: %v", err)
	}

	loaded, err := s.Load(path)
	if err != nil {
		t.Fatalf("Load: %v", err)
	}
	docID := loaded.DocID
	oldSaved, hasSaved, err := s.SavedObs(docID)
	if err != nil || !hasSaved {
		t.Fatalf("SavedObs after Load: hasSaved=%v err=%v", hasSaved, err)
	}

	// External write; Probe records a BARE sighting — no adoption.
	if err := mem.WriteFile(path, []byte("external"), 0o644); err != nil {
		t.Fatalf("external write: %v", err)
	}
	probed, err := s.Probe(docID)
	if err != nil {
		t.Fatalf("Probe: %v", err)
	}
	if probed.Kind != SyncDiskAhead {
		t.Fatalf("setup: Probe.Kind = %v, want SyncDiskAhead", probed.Kind)
	}

	// Sync (pure, no disk I/O) must classify against the FRESH sighting.
	synced, err := s.Sync(docID)
	if err != nil {
		t.Fatalf("Sync: %v", err)
	}
	if synced.Kind != SyncDiskAhead {
		t.Fatalf("Sync.Kind = %v, want SyncDiskAhead (against the fresh sighting, not the stale saved_obs)", synced.Kind)
	}
	if synced.Theirs.Hash != hashBytes([]byte("external")) {
		t.Fatalf("Sync.Theirs.Hash = %q, want hash of the FRESH sighting %q", synced.Theirs.Hash, "external")
	}

	// SavedObs must still return the OLD agreement — a bare Probe never
	// moves the CAS baseline.
	stillSaved, hasSaved2, err := s.SavedObs(docID)
	if err != nil || !hasSaved2 {
		t.Fatalf("SavedObs after Probe: hasSaved=%v err=%v", hasSaved2, err)
	}
	if stillSaved.ID != oldSaved.ID || stillSaved.BlobHash != hashBytes([]byte("agreed")) {
		t.Fatalf("SavedObs moved on a bare probe: got obs %d (%q), want the OLD agreement obs %d (%q)",
			stillSaved.ID, stillSaved.BlobHash, oldSaved.ID, oldSaved.BlobHash)
	}
}

// TestLoad_DivergedSightingDoesNotBecomeAncestor is the F2 PoC: seed -> Load
// -> local edit -> external write -> re-Load (Sync == Diverged) -> one more
// AppendEdit -> Sync MUST still be SyncDiverged, and a save with the current
// SavedObs as CAS expect must refuse. Pre-fix, Load unconditionally
// correlated its fresh (possibly diverged) sighting to the load-time seq and
// advanced saved_obs to it; once the undo position moved past that seq (the
// "one more" edit), the self-reference guard in ancestorAt no longer
// excluded it, so the diverged sighting became its OWN ancestor and Sync
// silently flipped Diverged -> BufferAhead, letting a save clobber theirs.
func TestLoad_DivergedSightingDoesNotBecomeAncestor(t *testing.T) {
	s, mem := newMatTestStore(t)
	const path = "/note.md"
	const ancestorContent = "ancestor\n"
	if err := mem.WriteFile(path, []byte(ancestorContent), 0o644); err != nil {
		t.Fatalf("seed file: %v", err)
	}

	first, err := s.Load(path)
	if err != nil {
		t.Fatalf("first Load: %v", err)
	}
	docID := first.DocID

	const localEdit = "local-edit\n"
	if _, err := s.AppendEdit(docID,
		[]buffer.AppliedEdit{{Start: 0, End: len(ancestorContent), Deleted: ancestorContent, Insert: localEdit}},
		nil, nil); err != nil {
		t.Fatalf("AppendEdit (local): %v", err)
	}

	const externalContent = "external-change\n"
	if err := mem.WriteFile(path, []byte(externalContent), 0o644); err != nil {
		t.Fatalf("external write: %v", err)
	}

	second, err := s.Load(path)
	if err != nil {
		t.Fatalf("second Load: %v", err)
	}
	if second.Sync.Kind != SyncDiverged {
		t.Fatalf("second Load: Sync.Kind = %v, want SyncDiverged", second.Sync.Kind)
	}

	// Esc + one keystroke, in the PoC's terms: one more local edit past the
	// diverged reload.
	const localEdit2 = "local-edit\nmore\n"
	if _, err := s.AppendEdit(docID,
		[]buffer.AppliedEdit{{Start: 0, End: len(localEdit), Deleted: localEdit, Insert: localEdit2}},
		nil, nil); err != nil {
		t.Fatalf("AppendEdit (one more): %v", err)
	}

	after, err := s.Sync(docID)
	if err != nil {
		t.Fatalf("Sync after one more local edit: %v", err)
	}
	if after.Kind != SyncDiverged {
		t.Fatalf("Sync after one more local edit: Kind = %v, want SyncDiverged (F2: a diverged sighting must never become an ancestor)", after.Kind)
	}

	expect, hasSaved, err := s.SavedObs(docID)
	if err != nil || !hasSaved {
		t.Fatalf("SavedObs: hasSaved=%v err=%v", hasSaved, err)
	}
	seq, err := s.CurrentSeq(docID)
	if err != nil {
		t.Fatalf("CurrentSeq: %v", err)
	}
	result, err := s.Materialize(docID, path, "clobbering content", expect.ID, seq, false)
	if err != nil {
		t.Fatalf("Materialize: %v", err)
	}
	if result.Committed {
		t.Fatal("Materialize(expect=SavedObs) must refuse (Committed=false) — a silent clobber of theirs")
	}
}

// TestLoad_HashEqualityHealAdopts: a crash-between-swap-and-ack (the physical
// write succeeded, Materialize's ack tx never committed) leaves disk hash ==
// the journal-head reconstruction on reopen. Load must heal-adopt: a fresh
// origin='resolve' observation correlated to head seq, saved_obs moved to
// it, and it must be selectable as the ancestor at that position.
func TestLoad_HashEqualityHealAdopts(t *testing.T) {
	s, mem := newMatTestStore(t)
	const path = "/note.md"
	if err := mem.WriteFile(path, []byte("original"), 0o644); err != nil {
		t.Fatalf("seed file: %v", err)
	}

	first, err := s.Load(path)
	if err != nil {
		t.Fatalf("first Load: %v", err)
	}
	docID := first.DocID

	const postCrash = "post-crash content"
	editSeq, err := s.AppendEdit(docID,
		[]buffer.AppliedEdit{{Start: 0, End: len("original"), Deleted: "original", Insert: postCrash}}, nil, nil)
	if err != nil {
		t.Fatalf("AppendEdit: %v", err)
	}

	// Simulate the physical swap succeeding without commitSave's ack landing.
	if err := mem.WriteFile(path, []byte(postCrash), 0o644); err != nil {
		t.Fatalf("simulate crash-write: %v", err)
	}

	second, err := s.Load(path)
	if err != nil {
		t.Fatalf("second Load: %v", err)
	}
	if second.Sync.Kind != SyncClean {
		t.Fatalf("second Load: Sync.Kind = %v, want SyncClean (hash-equality heal-adopt)", second.Sync.Kind)
	}

	saved, hasSaved, err := s.SavedObs(docID)
	if err != nil || !hasSaved {
		t.Fatalf("SavedObs: hasSaved=%v err=%v", hasSaved, err)
	}
	if saved.Origin != "resolve" {
		t.Fatalf("saved_obs.Origin = %q, want %q (heal-adopt)", saved.Origin, "resolve")
	}
	if !saved.Seq.Valid || saved.Seq.Int64 != editSeq {
		t.Fatalf("saved_obs.Seq = %+v, want a valid seq == %d (ancestor-eligible)", saved.Seq, editSeq)
	}

	anc, hasAnc, err := s.ancestorAt(docID, editSeq, 0)
	if err != nil || !hasAnc {
		t.Fatalf("ancestorAt(editSeq): hasAnc=%v err=%v", hasAnc, err)
	}
	if anc.ID != saved.ID {
		t.Fatalf("ancestorAt(editSeq) = observation %d, want the heal-adopted observation %d", anc.ID, saved.ID)
	}
}

// TestProbe_AutoAdoptIsAncestorEligible is the F6 regression: after Probe's
// hash-equality auto-adopt, one further local edit must classify as
// SyncBufferAhead (an ordinary unsaved edit against the auto-adopted
// ancestor) — never SyncDiverged. Pre-fix, auto-adopt moved saved_obs to a
// seq-NULL bare 'probe' observation that ancestorAt could never select, so
// the ORIGINAL load observation was found as ancestor instead, misclassifying
// the auto-adopted content itself as a divergence.
func TestProbe_AutoAdoptIsAncestorEligible(t *testing.T) {
	s, mem := newMatTestStore(t)
	const path = "/note.md"
	if err := mem.WriteFile(path, []byte("original"), 0o644); err != nil {
		t.Fatalf("seed file: %v", err)
	}
	loaded, err := s.Load(path)
	if err != nil {
		t.Fatalf("Load: %v", err)
	}
	docID := loaded.DocID

	const v2 = "v2 content"
	if _, err := s.AppendEdit(docID,
		[]buffer.AppliedEdit{{Start: 0, End: len("original"), Deleted: "original", Insert: v2}}, nil, nil); err != nil {
		t.Fatalf("AppendEdit v2: %v", err)
	}
	if err := mem.WriteFile(path, []byte(v2), 0o644); err != nil {
		t.Fatalf("write v2 externally: %v", err)
	}

	state, err := s.Probe(docID)
	if err != nil {
		t.Fatalf("Probe: %v", err)
	}
	if state.Kind != SyncClean {
		t.Fatalf("Probe: Kind = %v, want SyncClean (auto-adopt)", state.Kind)
	}

	const v3 = "v3 content"
	if _, err := s.AppendEdit(docID,
		[]buffer.AppliedEdit{{Start: 0, End: len(v2), Deleted: v2, Insert: v3}}, nil, nil); err != nil {
		t.Fatalf("AppendEdit v3: %v", err)
	}

	after, err := s.Sync(docID)
	if err != nil {
		t.Fatalf("Sync after edit past auto-adopt: %v", err)
	}
	if after.Kind != SyncBufferAhead {
		t.Fatalf("Sync after edit past auto-adopt: Kind = %v, want SyncBufferAhead (F6: the auto-adopted observation must be ancestor-eligible)", after.Kind)
	}
}

// TestResolveAbandon_ExactRestore is the F3 store-half regression: agreement
// P -> diverged reload sighting -> adopt-at-entry R (a resolution) -> a
// further journaled edit reverting the buffer (mirrors mergemode.Abort's
// pre-merge-ours restore) -> ResolveAbandon. SavedObs must land back on P
// EXACTLY (not re-derived by an origin scan, which the intervening diverged
// sighting would poison), and Sync must report SyncDiverged again — the
// abandon must never silently leave the doc looking Clean/resolved.
func TestResolveAbandon_ExactRestore(t *testing.T) {
	s, mem := newMatTestStore(t)
	const path = "/note.md"
	const ancestorContent = "ancestor\n"
	if err := mem.WriteFile(path, []byte(ancestorContent), 0o644); err != nil {
		t.Fatalf("seed file: %v", err)
	}

	loaded, err := s.Load(path)
	if err != nil {
		t.Fatalf("Load: %v", err)
	}
	docID := loaded.DocID
	agreementP, hasSaved, err := s.SavedObs(docID)
	if err != nil || !hasSaved {
		t.Fatalf("SavedObs after Load: hasSaved=%v err=%v", hasSaved, err)
	}

	const localEdit = "local-edit\n"
	if _, err := s.AppendEdit(docID,
		[]buffer.AppliedEdit{{Start: 0, End: len(ancestorContent), Deleted: ancestorContent, Insert: localEdit}}, nil, nil); err != nil {
		t.Fatalf("AppendEdit (local): %v", err)
	}

	const theirsContent = "theirs-content\n"
	if err := mem.WriteFile(path, []byte(theirsContent), 0o644); err != nil {
		t.Fatalf("external write: %v", err)
	}
	preResolve, err := s.Probe(docID)
	if err != nil {
		t.Fatalf("Probe: %v", err)
	}
	if preResolve.Kind != SyncDiverged {
		t.Fatalf("setup: Probe.Kind = %v, want SyncDiverged", preResolve.Kind)
	}
	freshObs := preResolve.Theirs.Obs

	// Adopt-at-entry: a discard-style resolution (mirrors applyDiscardConflict) —
	// journal the buffer to theirs exactly, then ResolveAdopt.
	resolveSeq, err := s.AppendEdit(docID,
		[]buffer.AppliedEdit{{Start: 0, End: len(localEdit), Deleted: localEdit, Insert: theirsContent}}, nil, nil)
	if err != nil {
		t.Fatalf("AppendEdit (resolve): %v", err)
	}
	if err := s.ResolveAdopt(docID, freshObs, resolveSeq); err != nil {
		t.Fatalf("ResolveAdopt: %v", err)
	}
	if clean, err := s.Sync(docID); err != nil || clean.Kind != SyncClean {
		t.Fatalf("setup: Sync after resolve: Kind=%v err=%v, want SyncClean", clean.Kind, err)
	}

	// Abort-mirror: a further journaled edit reverts the buffer back to the
	// pre-resolution local edit (mirrors mergemode.Abort's ReplaceAll(preMergeOurs)).
	if _, err := s.AppendEdit(docID,
		[]buffer.AppliedEdit{{Start: 0, End: len(theirsContent), Deleted: theirsContent, Insert: localEdit}}, nil, nil); err != nil {
		t.Fatalf("AppendEdit (abort-revert): %v", err)
	}

	if err := s.ResolveAbandon(docID); err != nil {
		t.Fatalf("ResolveAbandon: %v", err)
	}

	restored, hasSaved2, err := s.SavedObs(docID)
	if err != nil || !hasSaved2 {
		t.Fatalf("SavedObs after abandon: hasSaved=%v err=%v", hasSaved2, err)
	}
	if restored.ID != agreementP.ID {
		t.Fatalf("SavedObs after abandon = observation %d, want the EXACT original agreement %d (not the diverged sighting)", restored.ID, agreementP.ID)
	}

	after, err := s.Sync(docID)
	if err != nil {
		t.Fatalf("Sync after abandon: %v", err)
	}
	if after.Kind != SyncDiverged {
		t.Fatalf("Sync after abandon: Kind = %v, want SyncDiverged", after.Kind)
	}
}

// TestDirty_DiskAheadIsNotDirty: a pure external change (disk moved, buffer
// didn't) must never read as dirty. Pre-fix, IsDirty was Kind != Clean, which
// also flagged SyncDiskAhead as dirty. Reload (not Probe) triggers DiskAhead
// here: on d813fbc a bare Probe never advanced saved_obs, so Sync stayed
// blind to the external change (theirs still read as the OLD saved_obs,
// which equalled ours) and this scenario would coincidentally read Clean
// regardless of the IsDirty formula under test — masking the item-2 defect
// behind the separate B1 one. A second Load — which pre-fix unconditionally
// advanced saved_obs to whatever it just read — makes saved_obs itself
// reflect the external change, isolating the IsDirty-formula defect (Kind !=
// Clean vs. the correct BufferAhead-or-Diverged) on its own.
func TestDirty_DiskAheadIsNotDirty(t *testing.T) {
	s, mem := newMatTestStore(t)
	const path = "/note.md"
	if err := mem.WriteFile(path, []byte("original"), 0o644); err != nil {
		t.Fatalf("seed file: %v", err)
	}
	if _, err := s.Load(path); err != nil {
		t.Fatalf("first Load: %v", err)
	}

	if err := mem.WriteFile(path, []byte("changed externally"), 0o644); err != nil {
		t.Fatalf("external write: %v", err)
	}
	reloaded, err := s.Load(path)
	if err != nil {
		t.Fatalf("second Load: %v", err)
	}
	docID := reloaded.DocID
	if reloaded.Sync.Kind != SyncDiskAhead {
		t.Fatalf("setup: second Load Sync.Kind = %v, want SyncDiskAhead", reloaded.Sync.Kind)
	}

	dirty, err := s.IsDirty(docID)
	if err != nil {
		t.Fatalf("IsDirty: %v", err)
	}
	if dirty {
		t.Fatal("IsDirty = true for a pure external change (DiskAhead) with no local edit — want false")
	}
}
