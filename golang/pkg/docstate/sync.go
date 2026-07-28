package docstate

import (
	"fmt"
)

// ancestorAt derives the 3-way-merge ancestor for docID AT journal position
// pos, AS SEEN BY THIS SESSION (v10, B1 fix): the newest observation
// recorded by THIS session's own session_id, with origin IN
// ('load','save','resolve') and a correlated seq <= pos. This is NEVER a
// stored pointer (Part III) — undoing past a merge/discard's resolve
// observation moves pos back below it, and ancestorAt automatically returns
// an OLDER ancestor, re-exposing whatever divergence the resolution had
// settled. This is the structural replacement for the old
// handleMergeUnwindRead's bespoke re-detection logic.
//
// The session_id filter is what makes this session's own ⌘S/merge-resolve
// history the ONLY thing eligible to serve as ITS ancestor: without it, a
// DIFFERENT session's save/load/resolve observation is exactly as
// seq-correlated and origin-eligible as this session's own, and would
// silently become this session's ancestor too — collapsing `ancestor` onto
// `theirs` and making classifySync return BufferAhead instead of Diverged
// (the B1 blocker: a foreign session's save must never look like something
// THIS session already agreed to). newestObservation ("theirs") is the one
// query in this package that must stay unaffected by this — see its own
// doc comment.
//
// excludeObs, when non-zero, is excluded from the candidates IFF its own
// correlated seq equals pos exactly — never merely seq <= pos. This is the
// self-reference guard: theirs' own sighting (a 'load'/Probe-auto-adopted/
// resolve observation the CALLER is using as theirs) is correlated to the
// SAME position it is being compared FROM whenever the query happens at
// exactly that position (Load computing its own SyncState; any LATER Sync()
// call at that same still-current position, since the correlation is
// permanent once recorded) — that row can never legitimately serve as its
// own ancestor (a fact can't be its own history). Narrower than excluding
// the id outright: an OLDER correlation (seq < pos) for the SAME id (e.g.
// the doc hasn't been edited since) is a perfectly legitimate ancestor and
// must still be found — excluding by id alone would wrongly return "no
// ancestor" for the ordinary "load once, then edit" case.
func (s *Store) ancestorAt(docID int64, pos int64, excludeObs ObsID) (Observation, bool, error) {
	o, found, err := scanObservation(s.perm.QueryRow(
		`SELECT id, doc_id, session_id, blob_hash, seq, size, mtime, inode, device, nlink, origin, supersedes, at
		 FROM observations
		 WHERE doc_id=? AND session_id=? AND origin IN ('load','save','resolve') AND seq IS NOT NULL AND seq <= ?
		   AND (id != ? OR seq < ?)
		 ORDER BY seq DESC, id DESC LIMIT 1`,
		docID, s.sessionID, pos, int64(excludeObs), pos,
	))
	if err != nil {
		return Observation{}, false, fmt.Errorf("ancestor at doc %d pos %d: %w", docID, pos, err)
	}
	return o, found, nil
}

// Sync compares three recorded facts for docID — the buffer head
// (reconstructed from the journal via RecoverDocument), the freshest disk
// knowledge we have (theirs = the NEWEST recorded observation, any origin —
// package doc comment "Three facts"), and the derived 3-way-merge ancestor
// (ancestorAt) — and classifies the result. Pure SQLite: never touches disk
// (Update-safe). Probe is the disk-touching counterpart that records a fresh
// observation before doing the same comparison (making ITS OWN new
// observation the newest by construction). saved_obs plays no role here — it
// is read only by Materialize's CAS check (SavedObs) — so a bare sighting
// (an idle probe, a diverged reload) that never moved the CAS baseline is
// still visible to this classification, closing the B1 blind-spot a
// SavedObs-derived theirs would otherwise leave at load time and every idle
// probe tick.
func (s *Store) Sync(docID int64) (SyncState, error) {
	newest, hasNewest, err := s.newestObservation(docID)
	if err != nil {
		return SyncState{}, err
	}
	var theirs Version
	if hasNewest {
		theirs = Version{Hash: newest.BlobHash, Obs: newest.ID, Valid: true}
	}
	return s.syncWithTheirs(docID, theirs)
}

// syncWithTheirs runs the ours/ancestor reconstruction shared by Sync (theirs
// = the newest recorded observation, no disk I/O) and Probe (theirs = a
// just-recorded fresh observation, which IS the newest by construction the
// instant it's recorded).
func (s *Store) syncWithTheirs(docID int64, theirs Version) (SyncState, error) {
	pos, err := s.CurrentSeq(docID)
	if err != nil {
		return SyncState{}, err
	}
	oursContent, err := s.RecoverDocument(docID)
	if err != nil {
		return SyncState{}, err
	}
	ours := Version{Hash: hashBytes([]byte(oursContent)), Valid: true}

	var ancestor Version
	if anc, hasAnc, err := s.ancestorAt(docID, pos, theirs.Obs); err != nil {
		return SyncState{}, err
	} else if hasAnc {
		ancestor = Version{Hash: anc.BlobHash, Obs: anc.ID, Valid: true}
	}

	kind := classifySync(ancestor, ours, theirs)

	// Undo-unwind (Part III "Conflict lifecycle"): a resolve observation
	// (ResolveAdopt) correlates theirs to the edit seq that resolved it. If
	// the undo position has since moved BEHIND that seq, ancestorAt recomputes
	// an OLDER ancestor for pos — one the current (wound-back) buffer can
	// coincidentally match, which classifySync alone would read as the safe
	// "DiskAhead" auto-adopt case. It isn't: theirs was only ever reconciled
	// at a position we've since undone past, so the resolution's chain to
	// theirs no longer exists at pos — re-raise as Diverged, exactly
	// mirroring the old handleMergeUnwindRead's "undo past a resolution
	// re-raises the guard" behavior, now falling out of position-derived
	// state instead of bespoke re-detection logic.
	// The check is an EXISTS over ALL correlated observations — never a
	// property of whichever observation happens to be NEWEST: any bare
	// sighting recorded after the resolution (a flush-tick probe, a diverged
	// reload — both seq-NULL) becomes the newest observation and would
	// otherwise permanently defeat the override (review finding: undo past
	// an adoption, then ⌘S — classified DiskAhead, not even dirty, CAS
	// passes, and the adopted external bytes are clobbered with the
	// wound-back content). The error is surfaced, not swallowed (§1.3) — a
	// mis-read here feeds a save/quit gate decision.
	//
	// Scoped to session_id=s.sessionID (v10), matching ancestorAt's own
	// scoping and this check's own documented intent — "undo past a
	// resolution I MADE re-raises the guard": without the filter, a
	// DIFFERENT session's unrelated later activity on the same docID could
	// spuriously force one extra Diverged prompt on this session. This is a
	// precision fix, not a safety-critical one — the failure mode without it
	// is an unnecessary prompt, never data loss.
	if kind == SyncDiskAhead {
		var unwound bool
		if err := s.perm.QueryRow(
			`SELECT EXISTS(SELECT 1 FROM observations WHERE doc_id=? AND session_id=? AND seq IS NOT NULL AND seq > ?)`,
			docID, s.sessionID, pos,
		).Scan(&unwound); err != nil {
			return SyncState{}, fmt.Errorf("sync doc %d: unwind check: %w", docID, err)
		}
		if unwound {
			kind = SyncDiverged
		}
	}

	return SyncState{Kind: kind, Ancestor: ancestor, Ours: ours, Theirs: theirs}, nil
}

// emptyHash is the SHA-256 of the empty string — the "nothing to save yet"
// baseline for a document with no disk fact at all (an untitled scratch that
// was never loaded from or written to a real path).
var emptyHash = hashBytes(nil)

// classifySync implements the Conflict lifecycle comparison (Part III):
//   - No disk fact recorded yet (Theirs invalid, e.g. an untitled scratch):
//     Clean if the buffer is also empty (truly pristine — nothing to save);
//     otherwise BufferAhead (unsaved content with no disk counterpart at
//     all yet — this is what makes IsDirty correctly report an untitled
//     scratch with typed content as dirty, ancestor-derived: §1.4.8).
//   - Ours == Theirs: Clean, regardless of ancestor.
//   - No ancestor recorded but Ours != Theirs: Diverged — conservative,
//     since there is no history to reason from; never silently pick a side.
//   - Theirs == Ancestor (disk unchanged since the ancestor): BufferAhead —
//     an ordinary unsaved edit.
//   - Ours == Ancestor (buffer unchanged since the ancestor): DiskAhead — an
//     external edit landed while we made no local change.
//   - Otherwise: Diverged — both sides moved independently.
func classifySync(ancestor, ours, theirs Version) SyncKind {
	if !theirs.Valid {
		if ours.Hash == emptyHash {
			return SyncClean
		}
		return SyncBufferAhead
	}
	if ours.Hash == theirs.Hash {
		return SyncClean
	}
	if !ancestor.Valid {
		return SyncDiverged
	}
	switch {
	case theirs.Hash == ancestor.Hash:
		return SyncBufferAhead
	case ours.Hash == ancestor.Hash:
		return SyncDiskAhead
	default:
		return SyncDiverged
	}
}
