package docstate

import (
	"database/sql"
	"fmt"
	"os"
	"time"
)

// establishSession inserts a new `sessions` row for the CURRENT process and
// returns its id — called exactly once per Store construction (Open/OpenAt/
// OpenInMemory/NewTestStore), giving that Store its own private session
// identity (Store.sessionID). now is threaded in rather than reading
// time.Now() directly so OpenInMemory's caller-supplied clock (fuzz/tests)
// is honored even at construction time — this is the ONE record `opened_at`
// makes; it is informational bookkeeping only, never consulted by any
// liveness/inheritance/reaper decision (those use proc_started_at, and
// "most recent" is derived from events/snapshots seq — see load.go).
func establishSession(perm *sql.DB, now func() time.Time) (int64, error) {
	pid := os.Getpid()
	startedAt, _ := processStartedAt(pid) // ok ignored: "" is a valid (if degraded) value — ties this session to existence-only comparisons, never blocks session creation
	at := now().UTC().Format(time.RFC3339Nano)
	res, err := perm.Exec(
		`INSERT INTO sessions(pid, proc_started_at, opened_at) VALUES(?,?,?)`,
		pid, startedAt, at,
	)
	if err != nil {
		return 0, fmt.Errorf("establish session: %w", err)
	}
	id, err := res.LastInsertId()
	if err != nil {
		return 0, fmt.Errorf("establish session: last insert id: %w", err)
	}
	return id, nil
}

// isProcessAlive reports whether pid is still running the SAME process that
// was recorded with startedAt. Fails toward "alive" on any ambiguity (§0
// harm ladder): wrongly refusing to auto-inherit a recoverable draft is
// Tolerable (the user can still recover it manually via the scrubber/store);
// wrongly inheriting into and corrupting a still-live session's journal is
// not. Only a POSITIVE confirmation of death — the pid genuinely does not
// exist, or it exists but its recorded start time no longer matches (the OS
// recycled this pid to an unrelated process since) — returns false.
func isProcessAlive(pid int, startedAt string) bool {
	if pid <= 0 {
		return false // never a valid pid; a zero-value/corrupt row is never treated as a live blocker
	}
	exists, ok := processExists(pid)
	if !ok {
		return true // inconclusive existence check (e.g. sandboxed) — fail toward alive
	}
	if !exists {
		return false // positively confirmed: no such process
	}
	if startedAt == "" {
		return true // this session never captured a start time to compare — fail toward alive
	}
	current, ok := processStartedAt(pid)
	if !ok {
		return true // can't positively confirm identity right now — fail toward alive
	}
	return current == startedAt
}

// isSessionAlive reads sessionID's recorded (pid, proc_started_at) and
// applies s.livenessCheck. false with a nil error means the session row
// itself is already gone (can't be alive) — a defensive case the reaper's
// own safety condition (never reap a still-most-recent session) should make
// unreachable in practice, but a missing row is unambiguously "not alive"
// regardless.
func (s *Store) isSessionAlive(sessionID int64) (bool, error) {
	var pid int64
	var startedAt string
	err := s.perm.QueryRow(`SELECT pid, proc_started_at FROM sessions WHERE id=?`, sessionID).Scan(&pid, &startedAt)
	if err == sql.ErrNoRows {
		return false, nil
	}
	if err != nil {
		return false, fmt.Errorf("session %d liveness: %w", sessionID, err)
	}
	check := s.livenessCheck
	if check == nil {
		check = isProcessAlive
	}
	return check(int(pid), startedAt), nil
}

// ---- dead-session reaper (R1: bound the unbounded growth of
// sessions/session_documents/dead sessions' events without ever reaping
// content a future opener still needs to inherit) --------------------------

// reapDeadSessions runs once per Open/OpenAt (openStoreAt): for every
// sessions row confirmed dead by isAlive, deletes its session_documents/
// events/snapshots rows — but ONLY once it is no longer "the most recent
// session" (mostRecentSessionForDoc, load.go) for any docID it ever
// touched. Reaping the CURRENTLY-most-recent dead session for a doc would
// destroy the exact unsaved content the next opener still needs to inherit
// (Load's cross-session recovery) — this safety condition is the
// falsifiable part of "mitigate unbounded growth," not an implementation
// nicety.
//
// The `sessions` row itself is deliberately NEVER deleted here (unlike its
// session_documents/events/snapshots, which use real FK ON DELETE CASCADE
// from sessions — see store_schema.go): observations.session_id has NO
// cascade, by design, since a dead session's own save/load/resolve
// observation must remain a valid, visible "theirs" fact to every other
// session forever. Deleting the sessions row itself would either violate
// that FK (if the session ever recorded an observation — true for nearly
// every session that did anything) or require deleting those observations
// too, which would be a real correctness regression, not cleanup. Leaving a
// lean tombstone row behind (a handful of bytes: pid/proc_started_at/
// opened_at) is the accepted trade — R1's actual concern is the UNBOUNDED,
// content-bearing tables (event payloads, snapshot blobs), which this DOES
// reclaim.
//
// A session that is inherited but never built upon (the new session reads
// its content once but journals nothing further) can linger un-reaped until
// SOME session eventually journals a fresh edit for that doc — "most
// recent" is seq-based (load.go), and a freshly-created anchor snapshot at
// the new session's own local position 0 does not itself outrank the old
// session's higher raw seq numbers. This is a bounded, accepted trade for
// reusing ONE "most recent" mechanism between the inheritance decision and
// this safety check (rather than a second, independently-drifting notion of
// recency) — an efficiency question, never a correctness one: nothing about
// Sync/ancestorAt/Materialize's correctness depends on how promptly a
// session is reaped.
func reapDeadSessions(perm *sql.DB, isAlive func(pid int, startedAt string) bool) error {
	rows, err := perm.Query(`SELECT id, pid, proc_started_at FROM sessions`)
	if err != nil {
		return fmt.Errorf("reap dead sessions: list: %w", err)
	}
	type candidate struct {
		id        int64
		pid       int64
		startedAt string
	}
	var candidates []candidate
	for rows.Next() {
		var c candidate
		if err := rows.Scan(&c.id, &c.pid, &c.startedAt); err != nil {
			rows.Close()
			return fmt.Errorf("reap dead sessions: scan: %w", err)
		}
		candidates = append(candidates, c)
	}
	if err := rows.Err(); err != nil {
		rows.Close()
		return fmt.Errorf("reap dead sessions: rows: %w", err)
	}
	rows.Close()

	for _, c := range candidates {
		if isAlive(int(c.pid), c.startedAt) {
			continue
		}
		reapable, err := sessionIsReapable(perm, c.id)
		if err != nil {
			return fmt.Errorf("reap dead sessions: check session %d: %w", c.id, err)
		}
		if !reapable {
			continue
		}
		if err := reapSessionFootprint(perm, c.id); err != nil {
			return fmt.Errorf("reap dead sessions: reap session %d: %w", c.id, err)
		}
	}
	return nil
}

// sessionIsReapable reports whether sessionID is safe to reap: for EVERY
// docID it ever touched (via events or snapshots), some OTHER session must
// now hold the higher seq (mostRecentSessionForDoc) — i.e. sessionID is not
// "the most recent" for any doc it touched. A session that never touched any
// doc (vacuously true) is reapable.
func sessionIsReapable(perm *sql.DB, sessionID int64) (bool, error) {
	rows, err := perm.Query(`
		SELECT DISTINCT doc_id FROM (
			SELECT doc_id FROM events    WHERE session_id=?
			UNION
			SELECT doc_id FROM snapshots WHERE session_id=?
		)`, sessionID, sessionID)
	if err != nil {
		return false, fmt.Errorf("touched docs: %w", err)
	}
	var docIDs []int64
	for rows.Next() {
		var id int64
		if err := rows.Scan(&id); err != nil {
			rows.Close()
			return false, fmt.Errorf("scan doc: %w", err)
		}
		docIDs = append(docIDs, id)
	}
	if err := rows.Err(); err != nil {
		rows.Close()
		return false, fmt.Errorf("rows: %w", err)
	}
	rows.Close()

	for _, docID := range docIDs {
		mostRecent, found, err := mostRecentSessionForDoc(perm, docID)
		if err != nil {
			return false, fmt.Errorf("most recent session doc %d: %w", docID, err)
		}
		if found && mostRecent == sessionID {
			return false, nil // still the most-recent toucher of this doc — unsafe to reap
		}
	}
	return true, nil
}

// reapSessionFootprint deletes sessionID's session_documents/events/
// snapshots rows — its "footprint" — leaving the sessions row itself in
// place (see reapDeadSessions's doc comment for why). Explicit DELETEs
// rather than relying on cascading a sessions-row delete: this function
// deliberately never touches the sessions row, only what it produced.
func reapSessionFootprint(perm *sql.DB, sessionID int64) error {
	tx, err := perm.Begin()
	if err != nil {
		return fmt.Errorf("begin: %w", err)
	}
	if _, err := tx.Exec(`DELETE FROM session_documents WHERE session_id=?`, sessionID); err != nil {
		tx.Rollback() //nolint:errcheck
		return fmt.Errorf("delete session_documents: %w", err)
	}
	if _, err := tx.Exec(`DELETE FROM events WHERE session_id=?`, sessionID); err != nil {
		tx.Rollback() //nolint:errcheck
		return fmt.Errorf("delete events: %w", err)
	}
	if _, err := tx.Exec(`DELETE FROM snapshots WHERE session_id=?`, sessionID); err != nil {
		tx.Rollback() //nolint:errcheck
		return fmt.Errorf("delete snapshots: %w", err)
	}
	if err := tx.Commit(); err != nil {
		return fmt.Errorf("commit: %w", err)
	}
	return nil
}
