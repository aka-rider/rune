package docstate

import (
	"database/sql"
	"encoding/json"
	"fmt"
	"math"
	"time"
	"unicode/utf8"

	"rune/pkg/editor/buffer"
	"rune/pkg/editor/cursor"
)

// AppendEdit records an edit event in the in-memory journal.
// If len(edits)==0 it is a no-op.
// If the store's current position is not at the end of the journal
// (i.e. the user has undone some events), all abandoned future events
// are truncated before the new event is inserted.
// Adjacent single-character inserts on the same surface within 300 ms
// are coalesced into the previous event rather than producing a new one.
func (s *Store) AppendEdit(surface string, edits []buffer.AppliedEdit, cursorsBefore, cursorsAfter []cursor.Cursor, focusNow string) error {
	if len(edits) == 0 {
		return nil
	}

	// Truncate abandoned future when user has undone past events.
	if s.currentSeq != math.MaxInt64 {
		_, err := s.mem.Exec(`DELETE FROM events WHERE seq > ?`, s.currentSeq)
		if err != nil {
			return fmt.Errorf("truncate future events after seq %d: %w", s.currentSeq, err)
		}
		s.currentSeq = math.MaxInt64
	}

	now := s.clock().UTC().Format(time.RFC3339Nano)

	// Attempt coalescing with the previous event.
	if isInsertChar(edits) {
		var lastSeq int64
		var lastSurface, lastEditsJSON, lastKind, lastAt string
		err := s.mem.QueryRow(
			`SELECT seq, surface, kind, edits, at FROM events ORDER BY seq DESC LIMIT 1`,
		).Scan(&lastSeq, &lastSurface, &lastKind, &lastEditsJSON, &lastAt)

		if err == nil && lastKind == "edit" && lastSurface == surface {
			lastTime, parseErr := time.Parse(time.RFC3339Nano, lastAt)
			if parseErr == nil {
				elapsed := s.clock().UTC().Sub(lastTime)
				if elapsed <= 300*time.Millisecond && !lastInsertIsWhitespace(lastEditsJSON) {
					// Coalesce: merge new edits into the existing event.
					mergedJSON, mergeErr := mergeEditsJSON(lastEditsJSON, edits)
					if mergeErr != nil {
						return fmt.Errorf("coalesce edits for seq %d: %w", lastSeq, mergeErr)
					}

					newAfterJSON, marshalErr := json.Marshal(cursorsAfter)
					if marshalErr != nil {
						return fmt.Errorf("marshal cursors_after for coalesce: %w", marshalErr)
					}

					_, execErr := s.mem.Exec(
						`UPDATE events SET edits=?, cursors_after=?, at=? WHERE seq=?`,
						mergedJSON, string(newAfterJSON), now, lastSeq,
					)
					if execErr != nil {
						return fmt.Errorf("update coalesced event seq %d: %w", lastSeq, execErr)
					}
					return nil
				}
			}
		} else if err != nil && err != sql.ErrNoRows {
			return fmt.Errorf("query last event for coalesce check: %w", err)
		}
	}

	// Insert a new event.
	editsJSON, err := json.Marshal(edits)
	if err != nil {
		return fmt.Errorf("marshal edits: %w", err)
	}

	beforeJSON, err := json.Marshal(cursorsBefore)
	if err != nil {
		return fmt.Errorf("marshal cursors_before: %w", err)
	}

	afterJSON, err := json.Marshal(cursorsAfter)
	if err != nil {
		return fmt.Errorf("marshal cursors_after: %w", err)
	}

	_, err = s.mem.Exec(
		`INSERT INTO events(surface, kind, edits, cursors_before, cursors_after, focus_before, focus_after, is_undo_stop, anchor_snapshot_id, at)
		 VALUES(?, 'edit', ?, ?, ?, ?, ?, 1, 0, ?)`,
		surface,
		string(editsJSON),
		string(beforeJSON),
		string(afterJSON),
		focusNow,
		focusNow,
		now,
	)
	if err != nil {
		return fmt.Errorf("insert edit event for surface %q: %w", surface, err)
	}

	// s.currentSeq remains math.MaxInt64 — we're at the new end.
	return nil
}

// UndoTarget returns the most recent undo-stop event at or before the
// current journal position. On success it steps the position one behind
// the returned event so that a subsequent Undo call targets the event
// before it, and a Redo call can return to this one.
func (s *Store) UndoTarget() (surface string, edits []buffer.AppliedEdit, cursorsBefore []cursor.Cursor, ok bool) {
	var seq int64
	var editsJSON, cursorsJSON string

	err := s.mem.QueryRow(
		`SELECT seq, surface, edits, cursors_before FROM events
		 WHERE is_undo_stop=1 AND seq<=?
		 ORDER BY seq DESC LIMIT 1`,
		s.currentSeq,
	).Scan(&seq, &surface, &editsJSON, &cursorsJSON)

	if err == sql.ErrNoRows || err != nil {
		return "", nil, nil, false
	}

	if unmarshalErr := json.Unmarshal([]byte(editsJSON), &edits); unmarshalErr != nil {
		return "", nil, nil, false
	}

	if unmarshalErr := json.Unmarshal([]byte(cursorsJSON), &cursorsBefore); unmarshalErr != nil {
		return "", nil, nil, false
	}

	s.currentSeq = seq - 1
	return surface, edits, cursorsBefore, true
}

// RedoTarget returns the next undo-stop event after the current journal
// position. On success it advances the position to the returned event.
func (s *Store) RedoTarget() (surface string, edits []buffer.AppliedEdit, cursorsAfter []cursor.Cursor, ok bool) {
	if s.currentSeq == math.MaxInt64 {
		return "", nil, nil, false
	}

	var seq int64
	var editsJSON, cursorsJSON string

	err := s.mem.QueryRow(
		`SELECT seq, surface, edits, cursors_after FROM events
		 WHERE is_undo_stop=1 AND seq>?
		 ORDER BY seq ASC LIMIT 1`,
		s.currentSeq,
	).Scan(&seq, &surface, &editsJSON, &cursorsJSON)

	if err == sql.ErrNoRows || err != nil {
		return "", nil, nil, false
	}

	if unmarshalErr := json.Unmarshal([]byte(editsJSON), &edits); unmarshalErr != nil {
		return "", nil, nil, false
	}

	if unmarshalErr := json.Unmarshal([]byte(cursorsJSON), &cursorsAfter); unmarshalErr != nil {
		return "", nil, nil, false
	}

	s.currentSeq = seq
	return surface, edits, cursorsAfter, true
}

// isInsertChar reports whether edits represents a single character insertion
// (no deleted text, exactly one rune inserted).
func isInsertChar(edits []buffer.AppliedEdit) bool {
	if len(edits) != 1 {
		return false
	}
	e := edits[0]
	return e.Deleted == "" && utf8.RuneCountInString(e.Insert) == 1
}

// lastInsertIsWhitespace unmarshals editsJSON and reports whether the last
// edit's Insert text is a whitespace character (space, tab, newline, CR).
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

// mergeEditsJSON appends newEdits to the edits stored in existingJSON and
// returns the merged JSON blob.
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
