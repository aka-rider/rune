package docstate

import (
	"database/sql"
	"encoding/json"
	"fmt"
	"time"
	"unicode/utf8"

	"rune/pkg/editor/buffer"
	"rune/pkg/editor/cursor"
)

// Step is one undo/redo journal step: the edits to (re)apply to the buffer,
// the cursor state to restore, and the journal position to commit via
// MoveUndoPos once the buffer reapply succeeds. There is no surface field
// (I2: one document = one event stream) — the caller already knows which
// buffer a Step belongs to from the docID it peeked.
type Step struct {
	Edits   []buffer.AppliedEdit
	Cursors []cursor.Cursor
	NewPos  int64
}

// AppendEdit records an edit event in the durable journal for docID, tagged
// with this Store's own session_id (v10) — the journal key and the
// recovery/undo unit are the SAME document (I2: no surface dimension), and
// the journal AUTHOR is this session alone: every query below scopes to
// (doc_id, session_id) together, so a DIFFERENT session sharing this docID
// (two rune windows on the same file) can never see, coalesce with, or
// truncate this session's own events, and vice versa (the journal race the
// deleted global flock used to prevent by construction — CONSTITUTION.md
// §12). If len(edits)==0 it is a no-op (returns 0, nil). If THIS session's
// current_seq is non-NULL (it has undone some of its own events), future
// events are truncated before inserting the new one. Adjacent single-
// character inserts within 300 ms are coalesced into the previous event,
// but only when that previous event is this SAME session's own.
// Returns the journal seq of the inserted (or coalesced) event.
func (s *Store) AppendEdit(docID int64, edits []buffer.AppliedEdit, cursorsBefore, cursorsAfter []cursor.Cursor) (int64, error) {
	if len(edits) == 0 {
		return 0, nil
	}

	tx, err := s.perm.Begin()
	if err != nil {
		return 0, fmt.Errorf("append edit doc %d: begin tx: %w", docID, err)
	}

	// Read current undo position; NULL (or no row at all — this session has
	// never journaled anything for docID yet) means at head.
	var nullableCS sql.NullInt64
	if err := tx.QueryRow(`SELECT current_seq FROM session_documents WHERE session_id=? AND doc_id=?`, s.sessionID, docID).Scan(&nullableCS); err != nil && err != sql.ErrNoRows {
		tx.Rollback() //nolint:errcheck
		return 0, fmt.Errorf("append edit doc %d: read current_seq: %w", docID, err)
	}

	// Truncate abandoned future if the user had undone past events — scoped
	// to THIS session's own rows only; a different session's events at the
	// same doc_id are structurally invisible to this query.
	if nullableCS.Valid {
		if _, err := tx.Exec(
			`DELETE FROM events WHERE doc_id=? AND session_id=? AND seq > ?`,
			docID, s.sessionID, nullableCS.Int64,
		); err != nil {
			tx.Rollback() //nolint:errcheck
			return 0, fmt.Errorf("append edit doc %d: truncate future events: %w", docID, err)
		}
		// A snapshot anchored PAST the truncation point (RecoverDocument
		// correctness) describes content that only ever existed in the
		// future this new edit just abandoned — the events that led to it
		// are gone above, so it can never be legitimately reached by
		// undo/redo again. Left alive, it becomes a zombie anchor:
		// RecoverDocument picks "nearest snapshot AT OR BEFORE targetSeq",
		// and once THIS edit lands at a fresh (higher) seq, that stale
		// snapshot can be selected as the anchor with the truncated events
		// gone from the replay window — resurrecting abandoned-future bytes
		// UNDER the new edit instead of reconstructing it alone (verified:
		// TestAppendEdit_TruncationInvalidatesFutureSnapshots).
		if _, err := tx.Exec(
			`DELETE FROM snapshots WHERE doc_id=? AND session_id=? AND seq > ?`,
			docID, s.sessionID, nullableCS.Int64,
		); err != nil {
			tx.Rollback() //nolint:errcheck
			return 0, fmt.Errorf("append edit doc %d: truncate future snapshots: %w", docID, err)
		}
		if _, err := tx.Exec(`UPDATE session_documents SET current_seq=NULL WHERE session_id=? AND doc_id=?`, s.sessionID, docID); err != nil {
			tx.Rollback() //nolint:errcheck
			return 0, fmt.Errorf("append edit doc %d: reset current_seq: %w", docID, err)
		}
	}

	now := s.clock().UTC().Format(time.RFC3339Nano)

	// Attempt coalescing with the previous event for this doc — scoped to
	// this session's own rows, so a session only ever coalesces with its
	// own immediately-prior keystroke, never a different session's.
	if isInsertChar(edits) {
		var lastSeq int64
		var lastEditsJSON, lastAt string
		err := tx.QueryRow(
			`SELECT seq, edits, at FROM events WHERE doc_id=? AND session_id=? ORDER BY seq DESC LIMIT 1`,
			docID, s.sessionID,
		).Scan(&lastSeq, &lastEditsJSON, &lastAt)

		if err == nil {
			lastTime, parseErr := time.Parse(time.RFC3339Nano, lastAt)
			if parseErr == nil {
				elapsed := s.clock().UTC().Sub(lastTime)
				if elapsed <= 300*time.Millisecond && !lastInsertIsWhitespace(lastEditsJSON) {
					// Never mutate a row a snapshot has already anchored to
					// (RecoverDocument correctness): a snapshot at seq=lastSeq
					// freezes "content up to and including lastSeq" as of the
					// moment it was taken. RecoverDocument's replay window is
					// seq > anchorSnapshotSeq AND seq <= targetSeq — when the
					// anchor's own seq equals lastSeq, that window never
					// revisits the row AT lastSeq, so an UPDATE coalesced into
					// it after the snapshot exists is invisible to a
					// snapshot-anchored reconstruction even though the row
					// itself now holds the coalesced bytes — a silent replay
					// gap (verified: TestAppendEdit_NeverCoalescesIntoSnapshottedSeq).
					// Skip coalescing in that case; fall through to a new row.
					var snapshotExists bool
					if serr := tx.QueryRow(
						`SELECT EXISTS(SELECT 1 FROM snapshots WHERE doc_id=? AND seq=?)`,
						docID, lastSeq,
					).Scan(&snapshotExists); serr != nil {
						tx.Rollback() //nolint:errcheck
						return 0, fmt.Errorf("append edit doc %d: check snapshot at seq %d: %w", docID, lastSeq, serr)
					}
					if !snapshotExists {
						mergedJSON, mergeErr := mergeEditsJSON(lastEditsJSON, edits)
						if mergeErr != nil {
							tx.Rollback() //nolint:errcheck
							return 0, fmt.Errorf("coalesce edits for seq %d: %w", lastSeq, mergeErr)
						}
						newAfterJSON, marshalErr := json.Marshal(cursorsAfter)
						if marshalErr != nil {
							tx.Rollback() //nolint:errcheck
							return 0, fmt.Errorf("marshal cursors_after for coalesce: %w", marshalErr)
						}
						if _, execErr := tx.Exec(
							`UPDATE events SET edits=?, cursors_after=?, at=? WHERE seq=?`,
							mergedJSON, string(newAfterJSON), now, lastSeq,
						); execErr != nil {
							tx.Rollback() //nolint:errcheck
							return 0, fmt.Errorf("update coalesced event seq %d: %w", lastSeq, execErr)
						}
						if commitErr := tx.Commit(); commitErr != nil {
							return 0, fmt.Errorf("append edit: commit coalesce: %w", commitErr)
						}
						return lastSeq, nil
					}
				}
			}
		} else if err != sql.ErrNoRows {
			tx.Rollback() //nolint:errcheck
			return 0, fmt.Errorf("append edit doc %d: query last event for coalesce: %w", docID, err)
		}
	}

	// Marshal payload.
	editsJSON, err := json.Marshal(edits)
	if err != nil {
		tx.Rollback() //nolint:errcheck
		return 0, fmt.Errorf("append edit doc %d: marshal edits: %w", docID, err)
	}
	beforeJSON, err := json.Marshal(cursorsBefore)
	if err != nil {
		tx.Rollback() //nolint:errcheck
		return 0, fmt.Errorf("append edit doc %d: marshal cursors_before: %w", docID, err)
	}
	afterJSON, err := json.Marshal(cursorsAfter)
	if err != nil {
		tx.Rollback() //nolint:errcheck
		return 0, fmt.Errorf("append edit doc %d: marshal cursors_after: %w", docID, err)
	}

	// Insert the event. RETURNING seq reads the AUTOINCREMENT value.
	var newSeq int64
	if err := tx.QueryRow(
		`INSERT INTO events(doc_id, session_id, edits, cursors_before, cursors_after, at)
		 VALUES(?, ?, ?, ?, ?, ?)
		 RETURNING seq`,
		docID, s.sessionID, string(editsJSON), string(beforeJSON), string(afterJSON), now,
	).Scan(&newSeq); err != nil {
		tx.Rollback() //nolint:errcheck
		return 0, fmt.Errorf("append edit doc %d: insert event: %w", docID, err)
	}

	if err := tx.Commit(); err != nil {
		return 0, fmt.Errorf("append edit doc %d: commit: %w", docID, err)
	}
	return newSeq, nil
}

// UndoPeek returns the most recent event at or before the current journal
// position for docID, plus newPos — the position the journal should move to
// (one behind that event) once the buffer edit applies. It is READ-ONLY: it
// does NOT mutate current_seq. The caller commits the move via MoveUndoPos
// ONLY after the buffer reapply succeeds, so a failed apply never leaves
// current_seq ahead of the buffer (§1.4.8 coherence).
//
// ok=false means genuinely nothing to undo. A non-nil error means the read or
// an event's payload was corrupt — this is surfaced, never silently folded
// into ok=false (§1.3): a corrupt event is a Tolerable halt with the buffer
// kept, not "there was nothing to undo".
func (s *Store) UndoPeek(docID int64) (Step, bool, error) {
	// Determine position by reusing CurrentSeq's own
	// COALESCE(current_seq, MAX(seq), 0): "at head, some history" resolves to
	// MAX(seq) and "no history at all" resolves to 0 — and since real seqs
	// start at 1, a position of 0 can never match a row in the query below,
	// so the "nothing ever journaled" case falls through to the same
	// sql.ErrNoRows -> ok=false path as a genuinely-exhausted undo, with no
	// separate MAX(seq) query needed here.
	position, err := s.CurrentSeq(docID)
	if err != nil {
		return Step{}, false, fmt.Errorf("undo peek doc %d: %w", docID, err)
	}

	var seq int64
	var editsJSON, cursorsJSON string
	err = s.perm.QueryRow(
		`SELECT seq, edits, cursors_before FROM events
		 WHERE doc_id=? AND session_id=? AND seq<=?
		 ORDER BY seq DESC LIMIT 1`,
		docID, s.sessionID, position,
	).Scan(&seq, &editsJSON, &cursorsJSON)
	if err == sql.ErrNoRows {
		return Step{}, false, nil // genuinely nothing to undo
	}
	if err != nil {
		return Step{}, false, fmt.Errorf("undo peek doc %d: query event: %w", docID, err)
	}

	var edits []buffer.AppliedEdit
	if err := json.Unmarshal([]byte(editsJSON), &edits); err != nil {
		return Step{}, false, fmt.Errorf("undo peek doc %d: corrupt edits at seq %d: %w", docID, seq, err)
	}
	var cursors []cursor.Cursor
	if err := json.Unmarshal([]byte(cursorsJSON), &cursors); err != nil {
		return Step{}, false, fmt.Errorf("undo peek doc %d: corrupt cursors at seq %d: %w", docID, seq, err)
	}

	return Step{Edits: edits, Cursors: cursors, NewPos: seq - 1}, true, nil
}

// RedoPeek returns the next event after the current journal position for
// docID, plus newPos — the position the journal should advance to once the
// buffer edit applies. Like UndoPeek it is READ-ONLY; the caller commits via
// MoveUndoPos after a successful reapply. ok=false means genuinely nothing to
// redo; a non-nil error means the read or an event's payload was corrupt
// (surfaced, never folded into ok=false — §1.3).
func (s *Store) RedoPeek(docID int64) (Step, bool, error) {
	// Reuse CurrentSeq's position instead of re-deriving it here. At head
	// (current_seq NULL), CurrentSeq resolves to MAX(seq) when history
	// exists — and no event can ever have seq > MAX(seq), so the query below
	// naturally finds no rows and falls through to the same ok=false as the
	// "genuinely nothing to redo" case; with no history at all it resolves
	// to 0, and no event has seq > 0 either. Either way "at head" -> ok=false
	// without a separate short-circuit needed here.
	position, err := s.CurrentSeq(docID)
	if err != nil {
		return Step{}, false, fmt.Errorf("redo peek doc %d: %w", docID, err)
	}

	var seq int64
	var editsJSON, cursorsJSON string
	err = s.perm.QueryRow(
		`SELECT seq, edits, cursors_after FROM events
		 WHERE doc_id=? AND session_id=? AND seq>?
		 ORDER BY seq ASC LIMIT 1`,
		docID, s.sessionID, position,
	).Scan(&seq, &editsJSON, &cursorsJSON)
	if err == sql.ErrNoRows {
		return Step{}, false, nil // genuinely nothing to redo
	}
	if err != nil {
		return Step{}, false, fmt.Errorf("redo peek doc %d: query event: %w", docID, err)
	}

	var edits []buffer.AppliedEdit
	if err := json.Unmarshal([]byte(editsJSON), &edits); err != nil {
		return Step{}, false, fmt.Errorf("redo peek doc %d: corrupt edits at seq %d: %w", docID, seq, err)
	}
	var cursors []cursor.Cursor
	if err := json.Unmarshal([]byte(cursorsJSON), &cursors); err != nil {
		return Step{}, false, fmt.Errorf("redo peek doc %d: corrupt cursors at seq %d: %w", docID, seq, err)
	}

	return Step{Edits: edits, Cursors: cursors, NewPos: seq}, true, nil
}

// MoveUndoPos commits the journal undo position to pos for docID, scoped to
// this session (v10 — session_documents, PRIMARY KEY(session_id, doc_id)).
// Pair it with UndoPeek/RedoPeek: peek to read the target, apply the buffer
// edit, and only then MoveUndoPos — so a failed buffer reapply never leaves
// current_seq ahead of the buffer (§1.4.8). pos is always a concrete seq
// returned by UndoPeek/RedoPeek, matching the prior UndoTarget/RedoTarget
// behavior of writing a numeric position (functionally equivalent to NULL at
// head via COALESCE). The UPSERT creates this session's session_documents
// row on its very first undo/redo for docID — no read-then-write split (R3):
// pos is caller-supplied, never derived from a value this call itself reads.
func (s *Store) MoveUndoPos(docID, pos int64) error {
	if _, err := s.perm.Exec(
		`INSERT INTO session_documents(session_id, doc_id, current_seq) VALUES(?,?,?)
		 ON CONFLICT(session_id, doc_id) DO UPDATE SET current_seq=excluded.current_seq`,
		s.sessionID, docID, pos,
	); err != nil {
		return fmt.Errorf("move undo pos doc %d → %d: %w", docID, pos, err)
	}
	return nil
}

// readEditBatches runs query (a "SELECT edits FROM events WHERE ... ORDER BY
// seq ASC"-shaped query over the events table) and unmarshals each row's
// edits JSON into a []buffer.AppliedEdit batch, in row order. The shared
// body behind AllEdits and RecoverDocument's forward-replay gather — each
// independently ran this same Query/Scan/Unmarshal loop before this
// chokepoint, with only the WHERE clause differing. (EditsInRange needs each
// row's seq too, so it runs its own Query/Scan/Unmarshal loop instead of
// this one.)
func (s *Store) readEditBatches(query string, args ...any) ([][]buffer.AppliedEdit, error) {
	rows, err := s.perm.Query(query, args...)
	if err != nil {
		return nil, fmt.Errorf("query events: %w", err)
	}
	defer rows.Close()

	var result [][]buffer.AppliedEdit
	for rows.Next() {
		var editsJSON string
		if err := rows.Scan(&editsJSON); err != nil {
			return nil, fmt.Errorf("scan event edits: %w", err)
		}
		var batch []buffer.AppliedEdit
		if err := json.Unmarshal([]byte(editsJSON), &batch); err != nil {
			return nil, fmt.Errorf("unmarshal edits: %w", err)
		}
		result = append(result, batch)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("events rows: %w", err)
	}
	return result, nil
}

// AllEdits returns all edit events for the given docID in journal order. Each
// row in the events table becomes one inner slice.
// Used by the fuzz driver to rebuild the SHADOW mirror.
func (s *Store) AllEdits(docID int64) ([][]buffer.AppliedEdit, error) {
	result, err := s.readEditBatches(`SELECT edits FROM events WHERE doc_id=? ORDER BY seq ASC`, docID)
	if err != nil {
		return nil, fmt.Errorf("all edits doc %d: %w", docID, err)
	}
	return result, nil
}

// isInsertChar reports whether edits represents a single character insertion.
func isInsertChar(edits []buffer.AppliedEdit) bool {
	if len(edits) != 1 {
		return false
	}
	e := edits[0]
	return e.Deleted == "" && utf8.RuneCountInString(e.Insert) == 1
}

// lastInsertIsWhitespace reports whether the last edit in editsJSON is a
// whitespace character (space, tab, newline, CR).
func lastInsertIsWhitespace(editsJSON string) bool {
	var edits []buffer.AppliedEdit
	if err := json.Unmarshal([]byte(editsJSON), &edits); err != nil || len(edits) == 0 {
		return false
	}
	last := edits[len(edits)-1].Insert
	for _, r := range last {
		switch r {
		case ' ', '\t', '\n', '\r':
			return true
		}
	}
	return false
}

// mergeEditsJSON appends newEdits to the edits stored in existingJSON.
func mergeEditsJSON(existingJSON string, newEdits []buffer.AppliedEdit) (string, error) {
	var existing []buffer.AppliedEdit
	if err := json.Unmarshal([]byte(existingJSON), &existing); err != nil {
		return "", fmt.Errorf("unmarshal existing edits: %w", err)
	}
	merged := append(existing, newEdits...)
	b, err := json.Marshal(merged)
	if err != nil {
		return "", fmt.Errorf("marshal merged edits: %w", err)
	}
	return string(b), nil
}
