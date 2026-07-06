package docstate

import (
	"database/sql"
	"errors"
	"fmt"
	"io/fs"
)

// Probe refreshes a document's disk fact: unconditionally reads and hashes
// the live target (metadata is only ever a cheap hint — a stat happens
// first, but the content hash is what every comparison actually uses,
// closing G3), records the sighting as observation(origin='probe'), and
// returns the resulting SyncState. Cmd-only (touches disk) — the disk-
// refreshing counterpart to Sync's pure comparison of already-recorded
// facts.
//
// Unlike Materialize, Probe NEVER moves saved_obs on a bare divergence — a
// probe is passive observation, not consent to overwrite (silently adopting
// an external change here would be exactly the corruption class this plan
// closes). The ONE exception is auto-adopt: when the fresh read turns out to
// hash-equal the journal-head reconstruction (Kind == SyncClean), this is
// unambiguously OUR OWN prior write (most likely a Materialize whose ack tx
// never committed before a crash — step 7 of the Materialize protocol) and
// is promoted to a REAL adoption via AdoptEqual — a new origin='resolve'
// observation correlated to the head position, so it is ancestor-eligible
// (F6) — never left as a seq-NULL bare 'probe' sighting that ancestorAt could
// never select, which would silently re-raise a false Diverged guard the
// moment the next keystroke lands. Never masks an actual external change
// (which lands Ours != Theirs, in BufferAhead/DiskAhead/Diverged instead).
//
// A target that has gone missing (deleted, or its parent dir removed)
// returns an error wrapping fs.ErrNotExist — errors.Is(err, fs.ErrNotExist)
// is exactly the workspace layer's deleted-guard trigger (WP5), keeping that
// discriminant an ordinary Go error rather than a sentinel bolted onto
// SyncState (§1.7 — SyncKind's four values stay exactly what Part IV
// specifies).
func (s *Store) Probe(docID int64) (SyncState, error) {
	fsys := s.fsys()

	var path string
	if err := s.perm.QueryRow(`SELECT path FROM documents WHERE id=?`, docID).Scan(&path); err != nil {
		return SyncState{}, fmt.Errorf("probe doc %d: read path: %w", docID, err)
	}
	if path == "" {
		// Untitled/scratch/chat — nothing on disk to probe.
		return s.Sync(docID)
	}

	resolved, err := fsys.Resolve(path)
	if err != nil {
		return SyncState{}, fmt.Errorf("probe doc %d: resolve: %w", docID, err)
	}
	if _, statErr := fsys.Stat(resolved); statErr != nil {
		if errors.Is(statErr, fs.ErrNotExist) {
			return SyncState{}, fmt.Errorf("probe doc %d: %q: %w", docID, path, fs.ErrNotExist)
		}
		return SyncState{}, fmt.Errorf("probe doc %d: stat: %w", docID, statErr)
	}

	data, err := fsys.ReadFile(resolved)
	if err != nil {
		return SyncState{}, fmt.Errorf("probe doc %d: read: %w", docID, err)
	}
	hash, err := s.PutBlob(string(data))
	if err != nil {
		return SyncState{}, fmt.Errorf("probe doc %d: put blob: %w", docID, err)
	}
	fresh, err := s.observeFromStat(fsys, docID, resolved, hash, sql.NullInt64{}, "probe")
	if err != nil {
		return SyncState{}, fmt.Errorf("probe doc %d: record observation: %w", docID, err)
	}

	theirs := Version{Hash: fresh.BlobHash, Obs: fresh.ID, Valid: true}
	state, err := s.syncWithTheirs(docID, theirs)
	if err != nil {
		return SyncState{}, fmt.Errorf("probe doc %d: sync: %w", docID, err)
	}

	if state.Kind == SyncClean {
		// Auto-adopt only when there is something to heal: if the CAS
		// baseline already carries this exact hash, this is an ordinary
		// clean probe (every focus return / flush tick / watcher event) —
		// stacking a fresh 'resolve' adoption per tick would grow
		// observations and the supersedes chain unboundedly (review,
		// below-cap finding).
		cur, ok, err := s.SavedObs(docID)
		if err != nil {
			return SyncState{}, fmt.Errorf("probe doc %d: auto-adopt: read baseline: %w", docID, err)
		}
		if !ok || cur.BlobHash != fresh.BlobHash {
			pos, err := s.CurrentSeq(docID)
			if err != nil {
				return SyncState{}, fmt.Errorf("probe doc %d: auto-adopt: read current seq: %w", docID, err)
			}
			if _, err := s.AdoptEqual(docID, fresh.ID, pos); err != nil {
				return SyncState{}, fmt.Errorf("probe doc %d: auto-adopt: %w", docID, err)
			}
		}
	}
	return state, nil
}
