package docstate

import (
	"database/sql"
	"fmt"
	"time"
)

// ResolveAdopt commits a [D]iscard/[M]erge resolution (or an explicit
// hash-equality adopt): it re-tags the observation obs (already recorded —
// theirs was captured by I1 the moment it was read) as origin='resolve',
// correlated to editSeq (the seq of the journaled ReplaceAll/merge-entry edit
// that resolved it), and advances saved_obs to it — all in ONE short tx, pure
// SQLite. Undo past editSeq moves the journal position below this resolve
// observation, so ancestorAt automatically stops finding it and Sync reports
// Diverged again — the guard re-raises without any bespoke unwind logic. The
// new row's supersedes records whatever saved_obs held immediately before
// this call, so a later ResolveAbandon (an Esc-abort of the resolution) can
// restore it EXACTLY.
func (s *Store) ResolveAdopt(docID int64, obs ObsID, editSeq int64) error {
	source, err := s.getObservation(obs)
	if err != nil {
		return fmt.Errorf("resolve adopt doc %d: read source observation: %w", docID, err)
	}
	// Routed through recordAdoption (the shared one-tx primitive) instead of
	// reimplementing "read prior saved_obs, insert, advance saved_obs" inline.
	if _, err := s.recordAdoption(docID, source.BlobHash, source.Size, source.MTime, source.Inode, source.Device, source.NLink, "resolve", editSeq); err != nil {
		return fmt.Errorf("resolve adopt doc %d: %w", docID, err)
	}
	return nil
}

// ResolveAbandon reverses the resolve observation a merge/discard adoption
// (ResolveAdopt) or a heal-adopt created — the store half of an Esc-abort out
// of the merge resolver (F3). It reads docID's CURRENT saved_obs (expected to
// be the resolve/adoption observation the caller is unwinding), deletes that
// row (the blob itself is kept — history is never destroyed, only the fact
// that it was "agreed" is retracted), and restores saved_obs to EXACTLY what
// it superseded — never re-derived by an origin scan, which an intervening
// diverged 'load'/'probe' sighting recorded between the adoption and this
// abandon could poison into picking the wrong baseline (critic R4). With
// theirs now derived from the newest sighting (Sync's Adoption Contract),
// whatever external bytes this resolution reconciled are still visible after
// the deletion — Sync immediately re-reports Diverged (or whatever it
// reported before the resolution) with no separate re-derivation needed here.
// A docID with no saved_obs at all (never adopted anything) is a safe no-op.
func (s *Store) ResolveAbandon(docID int64) error {
	tx, err := s.perm.Begin()
	if err != nil {
		return fmt.Errorf("resolve abandon doc %d: begin: %w", docID, err)
	}
	var current sql.NullInt64
	err = tx.QueryRow(`SELECT saved_obs FROM session_documents WHERE session_id=? AND doc_id=?`, s.sessionID, docID).Scan(&current)
	if err == sql.ErrNoRows {
		tx.Rollback() //nolint:errcheck
		return nil    // no session_documents row at all — nothing adopted yet, safe no-op
	}
	if err != nil {
		tx.Rollback() //nolint:errcheck
		return fmt.Errorf("resolve abandon doc %d: read saved_obs: %w", docID, err)
	}
	if !current.Valid {
		tx.Rollback() //nolint:errcheck
		return nil    // nothing adopted yet — safe no-op
	}
	var supersedes sql.NullInt64
	var origin string
	if err := tx.QueryRow(`SELECT supersedes, origin FROM observations WHERE id=?`, current.Int64).Scan(&supersedes, &origin); err != nil {
		tx.Rollback() //nolint:errcheck
		return fmt.Errorf("resolve abandon doc %d: read supersedes of observation %d: %w", docID, current.Int64, err)
	}
	// Abandon unwinds a RESOLUTION and nothing else. If the merge-entry
	// ResolveAdopt failed (surfaced but the resolver was still entered), the
	// baseline still points at the last genuine 'save'/'load' agreement —
	// deleting THAT row would destroy real observation history and regress
	// the CAS baseline to an older supersedes (review finding: spurious
	// conflicts, or a write blessed against a baseline that reflects no real
	// agreement). Refuse; the caller surfaces the error.
	if origin != "resolve" {
		tx.Rollback() //nolint:errcheck
		return fmt.Errorf("resolve abandon doc %d: baseline observation %d has origin %q, not a resolve adoption — refusing to delete it", docID, current.Int64, origin)
	}
	// Move session_documents.saved_obs OFF the row being deleted FIRST —
	// saved_obs itself carries a FK to observations(id), so deleting the row
	// it still points at would violate that FK before the second statement
	// ever runs.
	if _, err := tx.Exec(`UPDATE session_documents SET saved_obs=? WHERE session_id=? AND doc_id=?`, supersedes, s.sessionID, docID); err != nil {
		tx.Rollback() //nolint:errcheck
		return fmt.Errorf("resolve abandon doc %d: restore saved_obs: %w", docID, err)
	}
	if _, err := tx.Exec(`DELETE FROM observations WHERE id=?`, current.Int64); err != nil {
		tx.Rollback() //nolint:errcheck
		return fmt.Errorf("resolve abandon doc %d: delete observation %d: %w", docID, current.Int64, err)
	}
	if err := tx.Commit(); err != nil {
		return fmt.Errorf("resolve abandon doc %d: commit: %w", docID, err)
	}
	return nil
}

// recordAdoptionTx is the shared one-tx BODY behind every path that moves
// saved_obs to a NEWLY-inserted observation with the given content/stat
// facts: a fresh row (origin, seq, blobHash, and the stat fields), tagged
// with THIS Store's own session_id (v10), is inserted, supersedes is set to
// whatever THIS SESSION's saved_obs held immediately before (NULL if none),
// and this session's session_documents.saved_obs advances to the new row.
// Runs entirely inside the CALLER's already-open tx — recordAdoption below
// wraps this in its own Begin/Commit for standalone callers; commitSave
// (materialize.go) calls it INLINE within its own single save tx, since
// observation + saved_obs + re-Bind must commit atomically together there
// and so cannot be split into a second, independent transaction. A *Store
// method (not a package-level function) so it can read s.sessionID — R3:
// the read (prior saved_obs) and the write (the UPSERT below) both run on
// the SAME tx the caller passed in, preserving the transactional discipline
// CONSTITUTION.md §12 names as what makes flock-free concurrency safe.
func (s *Store) recordAdoptionTx(tx *sql.Tx, docID int64, blobHash string, size int64, mtime string, inode, device, nlink sql.NullInt64, origin string, seq sql.NullInt64, at string) (Observation, error) {
	var supersedes sql.NullInt64
	err := tx.QueryRow(`SELECT saved_obs FROM session_documents WHERE session_id=? AND doc_id=?`, s.sessionID, docID).Scan(&supersedes)
	if err != nil && err != sql.ErrNoRows {
		return Observation{}, fmt.Errorf("read prior saved_obs: %w", err)
	}
	// err == sql.ErrNoRows: no session_documents row yet for THIS session and
	// docID — supersedes stays the zero value (Valid=false), exactly like a
	// never-before-adopted document's saved_obs being NULL under the old
	// documents-column design. The UPSERT below creates the row.
	res, err := tx.Exec(
		`INSERT INTO observations(doc_id, session_id, blob_hash, seq, size, mtime, inode, device, nlink, origin, supersedes, at)
		 VALUES(?,?,?,?,?,?,?,?,?,?,?,?)`,
		docID, s.sessionID, blobHash, seq, size, mtime, inode, device, nlink, origin, supersedes, at,
	)
	if err != nil {
		return Observation{}, fmt.Errorf("insert: %w", err)
	}
	newID, err := res.LastInsertId()
	if err != nil {
		return Observation{}, fmt.Errorf("last insert id: %w", err)
	}
	if _, err := tx.Exec(
		`INSERT INTO session_documents(session_id, doc_id, saved_obs) VALUES(?,?,?)
		 ON CONFLICT(session_id, doc_id) DO UPDATE SET saved_obs=excluded.saved_obs`,
		s.sessionID, docID, newID,
	); err != nil {
		return Observation{}, fmt.Errorf("update saved_obs: %w", err)
	}
	return Observation{
		ID: ObsID(newID), DocID: docID, SessionID: s.sessionID, BlobHash: blobHash, Seq: seq,
		Size: size, MTime: mtime, Inode: inode, Device: device, NLink: nlink,
		Origin: origin, Supersedes: supersedes, At: at,
	}, nil
}

// recordAdoption is the shared one-tx primitive behind every STANDALONE path
// that moves saved_obs to a newly-inserted observation — a reader never
// observes the new observation recorded without saved_obs already pointing
// at it, or vice versa. Shared by AdoptEqual (Probe's crash-recovery
// auto-adopt, F6), ResolveAdopt (a user-driven merge/discard resolution),
// and Load's own first-sighting/heal-adopt cases (F2 item 3). commitSave
// (materialize.go) is NOT a standalone caller — its observation/saved_obs
// move must commit in the SAME tx as its re-Bind, so it calls
// recordAdoptionTx directly inside its own already-open tx instead.
func (s *Store) recordAdoption(docID int64, blobHash string, size int64, mtime string, inode, device, nlink sql.NullInt64, origin string, seq int64) (Observation, error) {
	tx, err := s.perm.Begin()
	if err != nil {
		return Observation{}, fmt.Errorf("record adoption doc %d: begin: %w", docID, err)
	}
	at := s.clock().UTC().Format(time.RFC3339Nano)
	seqVal := sql.NullInt64{Int64: seq, Valid: true}
	obs, err := s.recordAdoptionTx(tx, docID, blobHash, size, mtime, inode, device, nlink, origin, seqVal, at)
	if err != nil {
		tx.Rollback() //nolint:errcheck
		return Observation{}, fmt.Errorf("record adoption doc %d: %w", docID, err)
	}
	if err := tx.Commit(); err != nil {
		return Observation{}, fmt.Errorf("record adoption doc %d: commit: %w", docID, err)
	}
	return obs, nil
}

// AdoptEqual promotes a bare sighting (obs, already recorded — e.g. Probe's
// bare 'probe' observation) to a genuine adoption when its content
// hash-equals the journal-head reconstruction: a NEW origin='resolve'
// observation is inserted, correlated to headSeq (making it
// ANCESTOR-ELIGIBLE, unlike the bare sighting it promotes — F6), and
// saved_obs advances to it. This is the crash-between-swap-and-ack recovery
// path (Materialize's step 5 ack never committed, but the physical swap
// already wrote what the journal head says) — never used for an ordinary
// divergence, which must surface as DiskAhead/Diverged instead.
func (s *Store) AdoptEqual(docID int64, obs ObsID, headSeq int64) (Observation, error) {
	source, err := s.getObservation(obs)
	if err != nil {
		return Observation{}, fmt.Errorf("adopt equal doc %d: read source observation: %w", docID, err)
	}
	adopted, err := s.recordAdoption(docID, source.BlobHash, source.Size, source.MTime, source.Inode, source.Device, source.NLink, "resolve", headSeq)
	if err != nil {
		return Observation{}, fmt.Errorf("adopt equal doc %d: %w", docID, err)
	}
	return adopted, nil
}
