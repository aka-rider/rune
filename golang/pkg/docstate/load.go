package docstate

import (
	"database/sql"
	"fmt"
	"time"

	"rune/pkg/editor/buffer"
)

// Load reads path fresh from disk, resolves its document identity, records
// the sighting, and returns everything the caller needs to decide how to
// display it: the raw disk bytes, the journal-reconstructed content (if the
// document has history), and the resulting SyncState. Cmd-only — every step
// below touches disk or commits a short SQLite write; no DB tx is ever held
// open across a vfs.FS call.
//
// The Adoption Contract (observation.go package doc comment) governs what
// happens to saved_obs here:
//   - First-ever load (no history yet): the load observation IS the
//     adoption — content becomes the recovery-anchor snapshot at head seq,
//     and saved_obs moves to this load observation (consistent by
//     construction: ours == theirs == the anchor).
//   - Reload with history, hash-equality (disk hash == the reconstruction at
//     head): a heal-adopt — the crash-between-swap-and-ack recovery case
//     (Materialize's step 5 ack never committed, but the physical swap
//     already wrote what the journal head says). Recorded as
//     origin='resolve', seq = head pos, and saved_obs moves to it — same
//     one-tx primitive Probe's auto-adopt uses (recordAdoption).
//   - Reload with history, hashes differ: a genuine bare sighting — recorded
//     with seq NULL (uncorrelated, never ancestor-eligible) and saved_obs
//     left UNTOUCHED. The stale CAS expectation then refuses a later
//     Materialize write even if every UI-level guard were bypassed (defense
//     in depth), and — because Sync's theirs is now the newest observation,
//     not saved_obs (the Adoption Contract) — THIS sighting is exactly what
//     the returned SyncState classifies against, so a real load-time
//     conflict is never silently swallowed.
func (s *Store) Load(path string) (LoadResult, error) {
	fsys := s.fsys()

	resolved, err := fsys.Resolve(path)
	if err != nil {
		return LoadResult{}, fmt.Errorf("load %q: resolve: %w", path, err)
	}
	data, err := fsys.ReadFile(resolved)
	if err != nil {
		return LoadResult{}, fmt.Errorf("load %q: read: %w", path, err)
	}
	content := string(data)

	ref, err := s.OpenPath(resolved)
	if err != nil {
		return LoadResult{}, fmt.Errorf("load %q: open path: %w", path, err)
	}

	hash, err := s.PutBlob(content)
	if err != nil {
		return LoadResult{}, fmt.Errorf("load %q: put blob: %w", path, err)
	}

	// The journal position AT LOAD TIME. A brand-new document loads at
	// position 0 (no events yet); reopening one with history correlates to
	// wherever its undo position already sits.
	loadSeq, err := s.CurrentSeq(ref.ID)
	if err != nil {
		return LoadResult{}, fmt.Errorf("load %q: read current seq: %w", path, err)
	}

	// D12/D13/§1.7: inode/device/nlink are sql.NullInt64, built from
	// vfs.FileID's/vfs.FileNLink's own out-of-band `ok` — a stat failure
	// records NULL, never a literal 0.
	size, mtime, inode, device, nlink := statIdentity(fsys, resolved)
	// HasHistory BEFORE recording anything below — it must reflect whether
	// this document had GENUINE prior journal/snapshot history (an earlier
	// session's edits, or an earlier Load in this one), not history this
	// very call is about to create.
	hasHistory, err := s.HasHistory(ref.ID)
	if err != nil {
		return LoadResult{}, fmt.Errorf("load %q: has history: %w", path, err)
	}

	// recovered is the journal reconstruction — computed once here (rather
	// than separately below) so the hash-equality heal-adopt check and the
	// LoadResult.Recovered field never diverge from the same read.
	recovered := content
	if hasHistory {
		recovered, err = s.RecoverDocument(ref.ID)
		if err != nil {
			return LoadResult{}, fmt.Errorf("load %q: recover document: %w", path, err)
		}
	}

	switch {
	case !hasHistory:
		// Cross-session crash recovery (v10, B2/R2): THIS session has never
		// touched ref.ID before (hasHistory is session-scoped) — before
		// anchoring on plain disk content (today's unmodified first-load
		// behavior), check whether a DIFFERENT, now-CONFIRMED-DEAD session
		// left dangling unsaved content for this doc that nobody has
		// inherited yet. This lookup MUST run before this session writes
		// ANYTHING of its own for ref.ID (the snapshot/adoption below) —
		// otherwise it would immediately find ITSELF as "the most recent
		// session" for a doc it hasn't touched yet.
		draftContent, inheriting, ferr := s.findInheritableDraft(ref.ID, content, hash)
		if ferr != nil {
			return LoadResult{}, fmt.Errorf("load %q: %w", path, ferr)
		}

		// First-ever load: the sighting IS the adoption — but the recovery
		// anchor MUST commit first. The adoption asserts "the journal
		// reconstruction at loadSeq equals this blob" (Adoption Contract
		// eligibility), which only holds once the anchor snapshot exists;
		// an adoption without an anchor makes RecoverDocument reconstruct
		// from empty while the CAS baseline matches disk — a later
		// quit/evict save would replace the full file with the near-empty
		// reconstruction (review finding: rung 1 under a fault the old
		// fire-and-forget comment labeled ignorable). A failed anchor
		// WITHOUT the adoption is merely a failed load — surfaced, retried
		// on the next open, nothing recorded that could mislead a save.
		//
		// The anchor and the adoption ALWAYS use disk content/hash, even
		// when inheriting below — this is what keeps the Adoption Contract
		// intact (the journal reconstruction AT loadSeq, position 0, must
		// equal the adoption's own blob) and what lets an immediate ⌘S of
		// the inherited draft succeed (Materialize's CAS check compares
		// live disk against exactly this saved_obs).
		if _, err := s.CreateSnapshot(ref.ID, content, loadSeq); err != nil {
			return LoadResult{}, fmt.Errorf("load %q: anchor snapshot: %w", path, err)
		}
		if _, err := s.recordAdoption(ref.ID, hash, size, mtime, inode, device, nlink, "load", loadSeq); err != nil {
			return LoadResult{}, fmt.Errorf("load %q: adopt first sighting: %w", path, err)
		}
		if inheriting {
			// Bridge the just-adopted disk content forward to the dead
			// session's draft via ONE synthetic "replace-all" edit, journaled
			// under THIS session's own session_id at the very next position —
			// exactly as if the user had just pasted the recovered draft in.
			// This is what makes RecoverDocument/Sync below see the draft as
			// "ours" (dirty, BufferAhead) while saved_obs (adopted above)
			// still correctly matches disk — no special case needed anywhere
			// else in Sync/ancestorAt/Materialize.
			bridge := []buffer.AppliedEdit{{Start: 0, End: len(content), Deleted: content, Insert: draftContent}}
			if _, err := s.AppendEdit(ref.ID, bridge, nil, nil); err != nil {
				return LoadResult{}, fmt.Errorf("load %q: journal inherited draft: %w", path, err)
			}
			recovered = draftContent
		}
	case hash == hashBytes([]byte(recovered)):
		// Reload, hash-equality: heal-adopt (crash-between-swap-and-ack
		// recovery) — origin='resolve', seq=head pos, saved_obs moves. Only
		// when there is actually something to heal: an ordinary clean tab
		// switch also lands here (disk == reconstruction == baseline), and
		// stacking a fresh adoption per switch would grow observations and
		// the supersedes chain unboundedly (review, below-cap finding).
		cur, ok, err := s.SavedObs(ref.ID)
		if err != nil {
			return LoadResult{}, fmt.Errorf("load %q: heal-adopt: read baseline: %w", path, err)
		}
		if !ok || cur.BlobHash != hash {
			if _, err := s.recordAdoption(ref.ID, hash, size, mtime, inode, device, nlink, "resolve", loadSeq); err != nil {
				return LoadResult{}, fmt.Errorf("load %q: heal-adopt: %w", path, err)
			}
		}
	default:
		// Reload, hashes differ: a bare, uncorrelated sighting. saved_obs
		// stays exactly where it was — this reload never moves agreement.
		at := s.clock().UTC().Format(time.RFC3339Nano)
		if _, err := s.recordObservation(ref.ID, hash, sql.NullInt64{}, size, mtime, inode, device, nlink, "load", at); err != nil {
			return LoadResult{}, fmt.Errorf("load %q: record observation: %w", path, err)
		}
	}

	// (The first-sighting recovery anchor is stamped INSIDE the !hasHistory
	// case above, BEFORE its adoption — never here, and never on a reload:
	// a reload re-stamp would overwrite the anchor with stale content, the
	// same class of pre-v4 load-time re-stamping bug this design avoids.)

	// Sync is safe to compute AFTER recording this sighting's own observation
	// above: ancestorAt structurally excludes an observation from serving as
	// its own ancestor whenever its correlated seq equals the query position
	// (see ancestorAt's doc comment) — the self-reference an adoption
	// recorded above (correlated to EXACTLY loadSeq, the same position Sync
	// queries at) would otherwise create is excluded there, not by ordering
	// calls here. A divergent bare sighting (seq NULL) is never ancestor-
	// eligible in the first place.
	sync, err := s.Sync(ref.ID)
	if err != nil {
		return LoadResult{}, fmt.Errorf("load %q: sync: %w", path, err)
	}

	return LoadResult{
		DocID:       ref.ID,
		RenamedFrom: ref.RenamedFrom,
		DiskContent: content,
		Recovered:   recovered,
		HasHistory:  hasHistory,
		Sync:        sync,
		NLink:       int(nlink.Int64), // LoadResult.NLink stays a plain int (0 when unavailable) — see its own doc comment
	}, nil
}

// mostRecentSessionForDoc returns the session_id attached to whichever
// row — across docID's events and snapshots together — carries the highest
// seq (v10, R2): NOT sessions.opened_at, which is recorded independently
// per process and is skew-prone across machines/containers, while seq is
// ONE shared AUTOINCREMENT counter for the whole events table, so it orders
// every session's activity unambiguously and skew-free. Ties (seq is a
// single counter, so a genuine tie can only happen between an events row
// and a snapshot row sharing the identical value — the same session's own
// anchor at the position it just journaled) break by higher session_id, per
// the plan. found=false means docID has no session-scoped activity
// recorded at all (a genuinely fresh document nobody has ever journaled
// to). Shared by Load's cross-session inheritance decision
// (findInheritableDraft) and the dead-session reaper's retention safety
// check (liveness.go, sessionIsReapable) — ONE mechanism, not two
// independently-drifting notions of "recent".
func mostRecentSessionForDoc(perm *sql.DB, docID int64) (sessionID int64, found bool, err error) {
	err = perm.QueryRow(`
		SELECT session_id FROM (
			SELECT session_id, seq FROM events    WHERE doc_id=?
			UNION ALL
			SELECT session_id, seq FROM snapshots WHERE doc_id=?
		)
		ORDER BY seq DESC, session_id DESC
		LIMIT 1`,
		docID, docID,
	).Scan(&sessionID)
	if err == sql.ErrNoRows {
		return 0, false, nil
	}
	if err != nil {
		return 0, false, fmt.Errorf("most recent session doc %d: %w", docID, err)
	}
	return sessionID, true, nil
}

// findInheritableDraft looks for a DIFFERENT, now-confirmed-dead session's
// unsaved content for docID (v10, B2/R2) — called ONLY from Load's
// !hasHistory branch, before this session has written anything of its own
// for docID, so mostRecentSessionForDoc can never accidentally find this
// session's own not-yet-written anchor. draft==diskContent with
// inheriting=false covers four "nothing to inherit" cases alike
// (Architecture → Load): no other session ever touched this doc, the most
// recent one is still alive (its private draft stays private — never
// auto-adopted), disk has moved since that dead session's own last-known
// baseline (below), or its content happens to hash-equal disk anyway
// (nothing to actually inherit).
//
// The baseline check (bug found in review, post-worker): it is NOT enough
// to compare the dead session's reconstruction against CURRENT disk — a
// session that loaded, typed nothing, and quit cleanly (or one that typed
// something and SAVED it before quitting) reconstructs to exactly what IT
// last saw as disk truth. If disk has since moved on for an entirely
// unrelated reason (an external tool, or a rune session that already came
// and went before this one), that reconstruction is not "unsaved work" —
// it is a stale mirror of a PAST disk state. Bridging it in anyway would
// classify this session as BufferAhead against content strictly OLDER than
// current disk, and an immediate save would silently discard the newer
// disk content: THIS session's own adoption is anchored on current disk,
// and disk has not moved since THIS session's own load, so Materialize's
// CAS check passes cleanly with no guard ever raised (a Catastrophic §0
// silent overwrite, reproduced empirically against the pre-fix code here).
// The fix: only treat the dead session's content as inheritable when disk
// has NOT moved since THAT session's own last-known baseline
// (session_documents.saved_obs) — i.e., nothing legitimate happened to the
// file between the dead session's departure and this session's arrival.
// When disk HAS moved beyond what the dead session ever saw, fall back to
// plain disk content instead: the dead session's real unsaved edits (if
// any) are not physically destroyed — still recoverable via the scrubber —
// trading a Tolerable "not auto-surfaced this once" down from a
// Catastrophic silent overwrite (§0 Harm Ladder).
func (s *Store) findInheritableDraft(docID int64, diskContent, diskHash string) (draft string, inheriting bool, err error) {
	otherSessionID, found, err := mostRecentSessionForDoc(s.perm, docID)
	if err != nil {
		return "", false, fmt.Errorf("find inheritable draft: %w", err)
	}
	if !found {
		return diskContent, false, nil
	}
	alive, err := s.isSessionAlive(otherSessionID)
	if err != nil {
		return "", false, fmt.Errorf("find inheritable draft: %w", err)
	}
	if alive {
		return diskContent, false, nil
	}
	otherBaseline, hasBaseline, err := s.savedObsFor(otherSessionID, docID)
	if err != nil {
		return "", false, fmt.Errorf("find inheritable draft: read dead session %d baseline: %w", otherSessionID, err)
	}
	if hasBaseline && otherBaseline.BlobHash != diskHash {
		return diskContent, false, nil // disk moved since the dead session's own last-known fact — not safe to bridge
	}
	recoveredDraft, err := s.recoverAt(docID, otherSessionID)
	if err != nil {
		return "", false, fmt.Errorf("find inheritable draft: recover dead session %d: %w", otherSessionID, err)
	}
	if hashBytes([]byte(recoveredDraft)) == diskHash {
		return diskContent, false, nil // the dead session never actually diverged from disk
	}
	return recoveredDraft, true, nil
}

// RecoverAcrossSessions reconstructs docID's content EITHER from this
// session's own history (the ordinary case) OR, when this session has never
// touched docID and a different, now-CONFIRMED-DEAD session left dangling
// unsaved content for it, from that dead session's own reconstruction (v10,
// B2) — the untitled/scratch-document counterpart to Load's cross-session
// inheritance: an untitled document has no backing file to fall back to at
// all, so Load's own "seed the anchor from raw disk" escape hatch does not
// exist here. Used by restoreScratch and showUntitled
// (pkg/ui/pages/workspace), replacing their former raw HasHistory+
// RecoverDocument pairs, both of which silently read a brand-new session's
// (trivially empty) reconstruction of a docID it had never itself touched.
//
// Unlike Load, this is a READ for display only — it deliberately writes NO
// anchor snapshot and records NO adoption/observation. If the user then
// edits, the ordinary journalEdit/AppendEdit path journals it under the
// CURRENT session's own session_id from that point on, so a LATER call's
// own (session-scoped) HasHistory becomes true and takes the first branch
// below — no special "claim this draft" step is needed.
//
// found=false covers both "nothing recorded for this doc, ever" and "the
// most recent other session is still alive" alike: an alive session's
// private, unsaved draft stays private, exactly like Load's disk-fallback
// case — restoreScratch simply does not offer the tab; showUntitled shows
// an empty buffer, exactly as it would for a genuinely-new untitled today.
//
// Synchronous (a local SQLite read plus a local liveness check, no network/
// blocking I/O) — consistent with restoreScratch/showUntitled already
// calling RecoverDocument/HasHistory synchronously today; no new async Cmd
// plumbing is needed.
func (s *Store) RecoverAcrossSessions(docID int64) (content string, found bool, err error) {
	has, err := s.HasHistory(docID)
	if err != nil {
		return "", false, fmt.Errorf("recover across sessions doc %d: %w", docID, err)
	}
	if has {
		content, err := s.RecoverDocument(docID)
		if err != nil {
			return "", false, fmt.Errorf("recover across sessions doc %d: %w", docID, err)
		}
		return content, true, nil
	}

	otherSessionID, foundOther, err := mostRecentSessionForDoc(s.perm, docID)
	if err != nil {
		return "", false, fmt.Errorf("recover across sessions doc %d: %w", docID, err)
	}
	if !foundOther {
		return "", false, nil
	}
	alive, err := s.isSessionAlive(otherSessionID)
	if err != nil {
		return "", false, fmt.Errorf("recover across sessions doc %d: %w", docID, err)
	}
	if alive {
		return "", false, nil // an alive session's private draft stays private
	}
	content, err = s.recoverAt(docID, otherSessionID)
	if err != nil {
		return "", false, fmt.Errorf("recover across sessions doc %d: recover dead session %d: %w", docID, otherSessionID, err)
	}
	return content, true, nil
}
