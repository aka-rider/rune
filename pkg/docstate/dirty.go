package docstate

import "fmt"

// IsDirty reports whether docID has unsaved changes: true iff a live event
// exists strictly between the saved and current positions (order-independent).
// This is robust to the global AUTOINCREMENT: the predicate never assumes the
// document's first event has seq=1.
//
// dirty ⟺ ∃ event with seq in (MIN(cur,saved), MAX(cur,saved)]
//
// where cur  = COALESCE(current_seq, MAX(events.seq), 0)
//
//	saved = COALESCE(saved_seq, 0)
func (s *Store) IsDirty(docID int64) (bool, error) {
	var isDirty bool
	err := s.perm.QueryRow(`
		WITH pos AS (
		  SELECT COALESCE(d.current_seq,
		                  (SELECT MAX(seq) FROM events WHERE doc_id = d.id), 0) AS cur,
		         COALESCE(d.saved_seq, 0)                                       AS saved
		  FROM documents d WHERE d.id = ?
		)
		SELECT EXISTS(
		  SELECT 1 FROM events, pos
		  WHERE events.doc_id = ?
		    AND events.seq >  MIN(pos.cur, pos.saved)
		    AND events.seq <= MAX(pos.cur, pos.saved)
		)`,
		docID, docID,
	).Scan(&isDirty)
	if err != nil {
		return false, fmt.Errorf("is dirty doc %d: %w", docID, err)
	}
	return isDirty, nil
}

// MarkSavedAt records seq as the saved baseline for docID — the journal position
// the bytes written to disk correspond to, captured SYNCHRONOUSLY at save-start
// (via CurrentSeq, co-atomic with the content) and threaded through
// FileSavedMsg.SavedSeq. It deliberately does NOT re-read the live head: while an
// async write is in flight the user can journal new edits that advance the head,
// and stamping that head would mark the file clean at a position the written bytes
// never reflected — silently swallowing the in-flight edits (§1.4.2/§1.4.8). The
// clean boundary advances only to the position actually persisted.
func (s *Store) MarkSavedAt(docID, seq int64) error {
	if _, err := s.perm.Exec(`UPDATE documents SET saved_seq = ? WHERE id = ?`, seq, docID); err != nil {
		return fmt.Errorf("mark saved doc %d at seq %d: %w", docID, seq, err)
	}
	return nil
}

// CurrentSeq returns the effective journal position for docID: the undo
// pointer (current_seq) if set, or MAX(events.seq), or 0 if no events.
// Used to tag VFS snapshots at content-capture time (never inside goroutines).
func (s *Store) CurrentSeq(docID int64) (int64, error) {
	var seq int64
	err := s.perm.QueryRow(`
		SELECT COALESCE(current_seq, (SELECT MAX(seq) FROM events WHERE doc_id = ?), 0)
		FROM documents WHERE id = ?`,
		docID, docID,
	).Scan(&seq)
	if err != nil {
		return 0, fmt.Errorf("current seq doc %d: %w", docID, err)
	}
	return seq, nil
}
