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

// AppendEdit records an edit event in the durable journal.
// If len(edits)==0 it is a no-op (returns 0, nil).
// If the document's current_seq is non-NULL (user has undone some events),
// future events are truncated before inserting the new one.
// Adjacent single-character inserts on the same doc+surface within 300 ms
// are coalesced into the previous event.
// Returns the journal seq of the inserted (or coalesced) event.
func (s *Store) AppendEdit(docID int64, surface string, edits []buffer.AppliedEdit, cursorsBefore, cursorsAfter []cursor.Cursor, focusNow string) (int64, error) {
	if len(edits) == 0 {
		return 0, nil
	}

	tx, err := s.perm.Begin()
	if err != nil {
		return 0, fmt.Errorf("append edit doc %d: begin tx: %w", docID, err)
	}

	// Read current undo position; NULL means at head.
	var nullableCS sql.NullInt64
	if err := tx.QueryRow(`SELECT current_seq FROM documents WHERE id=?`, docID).Scan(&nullableCS); err != nil {
		tx.Rollback() //nolint:errcheck
		return 0, fmt.Errorf("append edit doc %d: read current_seq: %w", docID, err)
	}

	// Truncate abandoned future if the user had undone past events.
	if nullableCS.Valid {
		if _, err := tx.Exec(
			`DELETE FROM events WHERE doc_id=? AND seq > ?`,
			docID, nullableCS.Int64,
		); err != nil {
			tx.Rollback() //nolint:errcheck
			return 0, fmt.Errorf("append edit doc %d: truncate future events: %w", docID, err)
		}
		if _, err := tx.Exec(`UPDATE documents SET current_seq=NULL WHERE id=?`, docID); err != nil {
			tx.Rollback() //nolint:errcheck
			return 0, fmt.Errorf("append edit doc %d: reset current_seq: %w", docID, err)
		}
	}

	now := s.clock().UTC().Format(time.RFC3339Nano)

	// Attempt coalescing with the previous event for this doc.
	if isInsertChar(edits) {
		var lastSeq int64
		var lastSurface, lastEditsJSON, lastKind, lastAt string
		err := tx.QueryRow(
			`SELECT seq, surface, kind, edits, at FROM events WHERE doc_id=? ORDER BY seq DESC LIMIT 1`,
			docID,
		).Scan(&lastSeq, &lastSurface, &lastKind, &lastEditsJSON, &lastAt)

		if err == nil && lastKind == "edit" && lastSurface == surface {
			lastTime, parseErr := time.Parse(time.RFC3339Nano, lastAt)
			if parseErr == nil {
				elapsed := s.clock().UTC().Sub(lastTime)
				if elapsed <= 300*time.Millisecond && !lastInsertIsWhitespace(lastEditsJSON) {
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
		} else if err != nil && err != sql.ErrNoRows {
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

	// Insert the event. anchor_snapshot_id resolved as a scalar subquery so we
	// never need a separate round-trip. RETURNING seq reads the AUTOINCREMENT value.
	var newSeq int64
	if err := tx.QueryRow(
		`INSERT INTO events(doc_id, surface, kind, edits, cursors_before, cursors_after, focus_before, focus_after, is_undo_stop, anchor_snapshot_id, at)
		 VALUES(?, ?, 'edit', ?, ?, ?, ?, ?, 1, (SELECT id FROM snapshots WHERE doc_id=? ORDER BY id DESC LIMIT 1), ?)
		 RETURNING seq`,
		docID, surface,
		string(editsJSON), string(beforeJSON), string(afterJSON),
		focusNow, focusNow,
		docID, now,
	).Scan(&newSeq); err != nil {
		tx.Rollback() //nolint:errcheck
		return 0, fmt.Errorf("append edit doc %d: insert event: %w", docID, err)
	}

	if err := tx.Commit(); err != nil {
		return 0, fmt.Errorf("append edit doc %d: commit: %w", docID, err)
	}
	return newSeq, nil
}

// UndoTarget returns the most recent undo-stop event at or before the current
// journal position for docID. On success it steps the position one behind the
// returned event so that a subsequent UndoTarget targets the event before it,
// and a RedoTarget can return to this one.
func (s *Store) UndoTarget(docID int64) (surface string, edits []buffer.AppliedEdit, cursorsBefore []cursor.Cursor, newPos int64, ok bool) {
	tx, err := s.perm.Begin()
	if err != nil {
		return "", nil, nil, 0, false
	}

	// Determine position: NULL = head (use MAX seq).
	var nullableCS sql.NullInt64
	if err := tx.QueryRow(`SELECT current_seq FROM documents WHERE id=?`, docID).Scan(&nullableCS); err != nil {
		tx.Rollback() //nolint:errcheck
		return "", nil, nil, 0, false
	}

	var position int64
	if nullableCS.Valid {
		position = nullableCS.Int64
	} else {
		var maxSeq sql.NullInt64
		if err := tx.QueryRow(`SELECT MAX(seq) FROM events WHERE doc_id=?`, docID).Scan(&maxSeq); err != nil || !maxSeq.Valid {
			tx.Rollback() //nolint:errcheck
			return "", nil, nil, 0, false
		}
		position = maxSeq.Int64
	}

	var seq int64
	var editsJSON, cursorsJSON string
	err = tx.QueryRow(
		`SELECT seq, surface, edits, cursors_before FROM events
		 WHERE doc_id=? AND is_undo_stop=1 AND seq<=?
		 ORDER BY seq DESC LIMIT 1`,
		docID, position,
	).Scan(&seq, &surface, &editsJSON, &cursorsJSON)
	if err == sql.ErrNoRows || err != nil {
		tx.Rollback() //nolint:errcheck
		return "", nil, nil, 0, false
	}

	if unmarshalErr := json.Unmarshal([]byte(editsJSON), &edits); unmarshalErr != nil {
		tx.Rollback() //nolint:errcheck
		return "", nil, nil, 0, false
	}
	if unmarshalErr := json.Unmarshal([]byte(cursorsJSON), &cursorsBefore); unmarshalErr != nil {
		tx.Rollback() //nolint:errcheck
		return "", nil, nil, 0, false
	}

	writtenPos := seq - 1
	if _, err := tx.Exec(`UPDATE documents SET current_seq=? WHERE id=?`, writtenPos, docID); err != nil {
		tx.Rollback() //nolint:errcheck
		return "", nil, nil, 0, false
	}

	if err := tx.Commit(); err != nil {
		return "", nil, nil, 0, false
	}
	return surface, edits, cursorsBefore, writtenPos, true
}

// RedoTarget returns the next undo-stop event after the current journal
// position for docID. On success it advances the position to the returned event.
func (s *Store) RedoTarget(docID int64) (surface string, edits []buffer.AppliedEdit, cursorsAfter []cursor.Cursor, newPos int64, ok bool) {
	tx, err := s.perm.Begin()
	if err != nil {
		return "", nil, nil, 0, false
	}

	var nullableCS sql.NullInt64
	if err := tx.QueryRow(`SELECT current_seq FROM documents WHERE id=?`, docID).Scan(&nullableCS); err != nil {
		tx.Rollback() //nolint:errcheck
		return "", nil, nil, 0, false
	}

	// NULL = at head → nothing to redo.
	if !nullableCS.Valid {
		tx.Rollback() //nolint:errcheck
		return "", nil, nil, 0, false
	}
	position := nullableCS.Int64

	var seq int64
	var editsJSON, cursorsJSON string
	err = tx.QueryRow(
		`SELECT seq, surface, edits, cursors_after FROM events
		 WHERE doc_id=? AND is_undo_stop=1 AND seq>?
		 ORDER BY seq ASC LIMIT 1`,
		docID, position,
	).Scan(&seq, &surface, &editsJSON, &cursorsJSON)
	if err == sql.ErrNoRows || err != nil {
		tx.Rollback() //nolint:errcheck
		return "", nil, nil, 0, false
	}

	if unmarshalErr := json.Unmarshal([]byte(editsJSON), &edits); unmarshalErr != nil {
		tx.Rollback() //nolint:errcheck
		return "", nil, nil, 0, false
	}
	if unmarshalErr := json.Unmarshal([]byte(cursorsJSON), &cursorsAfter); unmarshalErr != nil {
		tx.Rollback() //nolint:errcheck
		return "", nil, nil, 0, false
	}

	if _, err := tx.Exec(`UPDATE documents SET current_seq=? WHERE id=?`, seq, docID); err != nil {
		tx.Rollback() //nolint:errcheck
		return "", nil, nil, 0, false
	}

	if err := tx.Commit(); err != nil {
		return "", nil, nil, 0, false
	}
	return surface, edits, cursorsAfter, seq, true
}

// AllEdits returns all edit events for the given docID and surface in journal
// order. Each row in the events table becomes one inner slice.
// Used by the fuzz driver to rebuild the SHADOW mirror.
func (s *Store) AllEdits(docID int64, surface string) ([][]buffer.AppliedEdit, error) {
	rows, err := s.perm.Query(
		`SELECT edits FROM events WHERE doc_id=? AND surface=? AND kind='edit' ORDER BY seq ASC`,
		docID, surface,
	)
	if err != nil {
		return nil, fmt.Errorf("all edits doc %d surface %q: %w", docID, surface, err)
	}
	defer rows.Close()

	var result [][]buffer.AppliedEdit
	for rows.Next() {
		var editsJSON string
		if err := rows.Scan(&editsJSON); err != nil {
			return nil, fmt.Errorf("all edits scan doc %d surface %q: %w", docID, surface, err)
		}
		var batch []buffer.AppliedEdit
		if err := json.Unmarshal([]byte(editsJSON), &batch); err != nil {
			return nil, fmt.Errorf("all edits unmarshal doc %d surface %q: %w", docID, surface, err)
		}
		result = append(result, batch)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("all edits rows doc %d surface %q: %w", docID, surface, err)
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
