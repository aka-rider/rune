package docstate

import "fmt"

// IsDirty reports whether docID has unsaved changes: dirty ⟺ ours (the
// journal reconstruction) differs from the derived 3-way-merge ancestor —
// SyncBufferAhead or SyncDiverged. NEVER Kind != Clean: that would also flag
// SyncDiskAhead (disk moved, the buffer didn't — a pure external change with
// nothing of the user's unsaved) as phantom-dirty. Ancestor-derived (WP5,
// §1.4.8 — recomputed from the store on every call, never a cached flag).
func (s *Store) IsDirty(docID int64) (bool, error) {
	state, err := s.Sync(docID)
	if err != nil {
		return false, fmt.Errorf("is dirty doc %d: %w", docID, err)
	}
	return state.Kind == SyncBufferAhead || state.Kind == SyncDiverged, nil
}

// DirtyDocs reports dirty status for every id in ids, each ONE INDEPENDENTLY
// recomputed from the store (never a cached per-tab flag) — closes H3: a
// background tab whose cached "clean" flag went stale is caught here because
// every id is re-derived from the durable journal/observation state at
// quit/evict decision time (§1.4.8). Unknown/missing ids are simply absent
// from the returned map — never defaulted to false or true (§1.7); a caller
// that needs a definite answer checks the map's second return.
func (s *Store) DirtyDocs(ids []int64) (map[int64]bool, error) {
	result := make(map[int64]bool, len(ids))
	for _, id := range ids {
		dirty, err := s.IsDirty(id)
		if err != nil {
			return nil, fmt.Errorf("dirty docs: doc %d: %w", id, err)
		}
		result[id] = dirty
	}
	return result, nil
}

// CurrentSeq returns the effective journal position for docID AS SEEN BY
// THIS SESSION (v10): this session's own undo pointer
// (session_documents.current_seq) if set, or MAX(events.seq) among only
// this session's own events for docID, or 0 if this session has no events
// for docID at all — a brand-new session on a docID with lots of history
// under OTHER sessions still starts at 0, exactly like a genuinely fresh
// document. Used to tag VFS snapshots at content-capture time (never inside
// goroutines).
func (s *Store) CurrentSeq(docID int64) (int64, error) {
	var seq int64
	err := s.perm.QueryRow(`
		SELECT COALESCE(
			(SELECT current_seq FROM session_documents WHERE session_id = ? AND doc_id = ?),
			(SELECT MAX(seq) FROM events WHERE doc_id = ? AND session_id = ?),
			0)`,
		s.sessionID, docID, docID, s.sessionID,
	).Scan(&seq)
	if err != nil {
		return 0, fmt.Errorf("current seq doc %d: %w", docID, err)
	}
	return seq, nil
}
