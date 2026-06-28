package docstate

import (
	"database/sql"
	"fmt"
	"time"
)

// ---- documents --------------------------------------------------------------

// OpenPath resolves the VFS document for a file that exists on disk.
// It must only be called after the file has been successfully read (so stat
// can obtain a real inode). Returns a DocRef with the stable document ID;
// RenamedFrom is set when the file was renamed since the VFS last saw it.
func (s *Store) OpenPath(path string) (DocRef, error) {
	at := s.clock().UTC().Format(time.RFC3339Nano)
	inode, device, ok := s.statID(path)

	if !ok || inode == 0 {
		return s.openPathByName(path, at)
	}
	return s.openPathByInode(path, inode, device, at)
}

// openPathByName is the path-keying fallback used when inode is unavailable.
func (s *Store) openPathByName(path, at string) (DocRef, error) {
	var id int64
	err := s.perm.QueryRow(
		`SELECT id FROM documents WHERE path=? AND (inode IS NULL OR inode=0)`,
		path,
	).Scan(&id)
	if err == sql.ErrNoRows {
		res, err := s.perm.Exec(
			`INSERT INTO documents(path, inode, device, created_at, last_seen_at) VALUES(?,0,0,?,?)`,
			path, at, at,
		)
		if err != nil {
			return DocRef{}, fmt.Errorf("open path %q: insert: %w", path, err)
		}
		id, err = res.LastInsertId()
		if err != nil {
			return DocRef{}, fmt.Errorf("open path %q: last insert id: %w", path, err)
		}
		return DocRef{ID: id}, nil
	}
	if err != nil {
		return DocRef{}, fmt.Errorf("open path %q: query: %w", path, err)
	}
	if _, err := s.perm.Exec(`UPDATE documents SET last_seen_at=? WHERE id=?`, at, id); err != nil {
		return DocRef{}, fmt.Errorf("open path %q: update last_seen_at: %w", path, err)
	}
	return DocRef{ID: id}, nil
}

func (s *Store) openPathByInode(path string, inode, device uint64, at string) (DocRef, error) {
	var rowID int64
	var rowPath string
	err := s.perm.QueryRow(
		`SELECT id, path FROM documents WHERE inode=? AND device=?`,
		inode, device,
	).Scan(&rowID, &rowPath)

	if err == sql.ErrNoRows {
		// New inode: evict any stale path holder and insert fresh row.
		tx, txErr := s.perm.Begin()
		if txErr != nil {
			return DocRef{}, fmt.Errorf("open path %q: begin tx: %w", path, txErr)
		}
		if _, execErr := tx.Exec(
			`UPDATE documents SET path='' WHERE path=? AND (inode IS NULL OR inode!=?)`,
			path, inode,
		); execErr != nil {
			tx.Rollback() //nolint:errcheck
			return DocRef{}, fmt.Errorf("open path %q: evict stale holder: %w", path, execErr)
		}
		res, execErr := tx.Exec(
			`INSERT INTO documents(path, inode, device, created_at, last_seen_at) VALUES(?,?,?,?,?)`,
			path, inode, device, at, at,
		)
		if execErr != nil {
			tx.Rollback() //nolint:errcheck
			return DocRef{}, fmt.Errorf("open path %q: insert: %w", path, execErr)
		}
		newID, execErr := res.LastInsertId()
		if execErr != nil {
			tx.Rollback() //nolint:errcheck
			return DocRef{}, fmt.Errorf("open path %q: last insert id: %w", path, execErr)
		}
		if commitErr := tx.Commit(); commitErr != nil {
			return DocRef{}, fmt.Errorf("open path %q: commit: %w", path, commitErr)
		}
		return DocRef{ID: newID}, nil
	}
	if err != nil {
		return DocRef{}, fmt.Errorf("open path %q: query by inode: %w", path, err)
	}

	// Found by inode.
	var renamedFrom string
	if rowPath != path {
		renamedFrom = rowPath
		tx, txErr := s.perm.Begin()
		if txErr != nil {
			return DocRef{}, fmt.Errorf("open path %q: begin rename tx: %w", path, txErr)
		}
		// Free any other row that claims the new path.
		if _, execErr := tx.Exec(
			`UPDATE documents SET path='' WHERE path=? AND id!=?`,
			path, rowID,
		); execErr != nil {
			tx.Rollback() //nolint:errcheck
			return DocRef{}, fmt.Errorf("open path %q: free old holder: %w", path, execErr)
		}
		if _, execErr := tx.Exec(
			`UPDATE documents SET path=?, inode=?, device=?, last_seen_at=? WHERE id=?`,
			path, inode, device, at, rowID,
		); execErr != nil {
			tx.Rollback() //nolint:errcheck
			return DocRef{}, fmt.Errorf("open path %q: rebind rename: %w", path, execErr)
		}
		if commitErr := tx.Commit(); commitErr != nil {
			return DocRef{}, fmt.Errorf("open path %q: commit rename: %w", path, commitErr)
		}
	} else {
		if _, err := s.perm.Exec(`UPDATE documents SET last_seen_at=? WHERE id=?`, at, rowID); err != nil {
			return DocRef{}, fmt.Errorf("open path %q: update last_seen_at: %w", path, err)
		}
	}
	return DocRef{ID: rowID, RenamedFrom: renamedFrom}, nil
}

// CreateScratch inserts a new unbound (untitled) VFS document and returns its
// DocRef. The display name is managed by the caller (workspace title component).
func (s *Store) CreateScratch(_ string) (DocRef, error) {
	at := s.clock().UTC().Format(time.RFC3339Nano)
	res, err := s.perm.Exec(
		`INSERT INTO documents(path, inode, device, created_at, last_seen_at) VALUES('',0,0,?,?)`,
		at, at,
	)
	if err != nil {
		return DocRef{}, fmt.Errorf("create scratch: insert: %w", err)
	}
	id, err := res.LastInsertId()
	if err != nil {
		return DocRef{}, fmt.Errorf("create scratch: last insert id: %w", err)
	}
	return DocRef{ID: id}, nil
}

// ReserveChatDoc returns the stable ID of the per-session chat document.
// It uses a sentinel path ("\x00chat") that can never name a real file.
// Events from the previous session are truncated so each launch starts clean.
func (s *Store) ReserveChatDoc() (int64, error) {
	const sentinel = "\x00chat"
	at := s.clock().UTC().Format(time.RFC3339Nano)

	tx, err := s.perm.Begin()
	if err != nil {
		return 0, fmt.Errorf("reserve chat doc: begin: %w", err)
	}
	if _, err := tx.Exec(
		`INSERT OR IGNORE INTO documents(path, inode, device, created_at, last_seen_at) VALUES(?,0,0,?,?)`,
		sentinel, at, at,
	); err != nil {
		tx.Rollback() //nolint:errcheck
		return 0, fmt.Errorf("reserve chat doc: insert sentinel: %w", err)
	}
	var id int64
	if err := tx.QueryRow(`SELECT id FROM documents WHERE path=?`, sentinel).Scan(&id); err != nil {
		tx.Rollback() //nolint:errcheck
		return 0, fmt.Errorf("reserve chat doc: select id: %w", err)
	}
	if _, err := tx.Exec(`DELETE FROM events WHERE doc_id=?`, id); err != nil {
		tx.Rollback() //nolint:errcheck
		return 0, fmt.Errorf("reserve chat doc: truncate events: %w", err)
	}
	if err := tx.Commit(); err != nil {
		return 0, fmt.Errorf("reserve chat doc: commit: %w", err)
	}
	return id, nil
}

// Bind (re)binds document docID to path, adopting the file's CURRENT inode/
// device and preserving the document id (and thus its full undo history). It is
// called in two places:
//
//   - first materialize of an untitled doc (naming / first save) — adopts the
//     freshly created file's inode;
//   - after EVERY overwrite save — the atomic write (temp→rename) gives the file
//     a NEW inode, so the recorded inode goes stale. Without re-binding, the next
//     OpenPath sees a "new inode at this path", evicts this row to path=” and
//     creates a fresh history-less doc — orphaning the undo DAG (§1.4.6) and
//     leaving a zombie row. Re-binding on save keeps identity stable across the
//     inode churn.
//
// Conflicting holders of the path or the new inode are evicted first so the
// unique indexes hold.
func (s *Store) Bind(docID int64, path string) error {
	at := s.clock().UTC().Format(time.RFC3339Nano)
	inode, device, ok := s.statID(path)

	tx, err := s.perm.Begin()
	if err != nil {
		return fmt.Errorf("bind %d → %q: begin: %w", docID, path, err)
	}
	// Free any other row holding this path (stale binding).
	if _, err := tx.Exec(`UPDATE documents SET path='' WHERE path=? AND id!=?`, path, docID); err != nil {
		tx.Rollback() //nolint:errcheck
		return fmt.Errorf("bind %d: free path holder: %w", docID, err)
	}
	if ok && inode != 0 {
		// Evict any stale row claiming this inode (deleted+recreated, or our own
		// prior inode reused by the filesystem).
		if _, err := tx.Exec(
			`UPDATE documents SET inode=0, device=0 WHERE inode=? AND device=? AND id!=?`,
			inode, device, docID,
		); err != nil {
			tx.Rollback() //nolint:errcheck
			return fmt.Errorf("bind %d: evict inode holder: %w", docID, err)
		}
		if _, err := tx.Exec(
			`UPDATE documents SET path=?, inode=?, device=?, last_seen_at=? WHERE id=?`,
			path, inode, device, at, docID,
		); err != nil {
			tx.Rollback() //nolint:errcheck
			return fmt.Errorf("bind %d: rebind: %w", docID, err)
		}
	} else {
		if _, err := tx.Exec(
			`UPDATE documents SET path=?, last_seen_at=? WHERE id=?`,
			path, at, docID,
		); err != nil {
			tx.Rollback() //nolint:errcheck
			return fmt.Errorf("bind %d: rebind by path: %w", docID, err)
		}
	}
	if err := tx.Commit(); err != nil {
		return fmt.Errorf("bind %d: commit: %w", docID, err)
	}
	return nil
}

// DeleteDoc removes a document and its journal/snapshots from the VFS. Used
// when the user explicitly discards an untitled buffer so it is not offered for
// recovery on the next launch. ON DELETE CASCADE removes the child events and
// snapshots rows automatically. Orphaned blobs are left for a future blob GC.
func (s *Store) DeleteDoc(docID int64) error {
	if _, err := s.perm.Exec(`DELETE FROM documents WHERE id=?`, docID); err != nil {
		return fmt.Errorf("delete doc %d: %w", docID, err)
	}
	return nil
}

// GCEmptyScratch deletes unbound (untitled) documents that carry neither events
// nor snapshots — empty scratch rows left over from prior sessions. keepID is
// never deleted (the live untitled buffer). Returns the number of rows removed.
// The chat sentinel has a non-empty path, so it is never affected.
func (s *Store) GCEmptyScratch(keepID int64) (int64, error) {
	res, err := s.perm.Exec(`
		DELETE FROM documents
		WHERE path='' AND id!=?
		  AND id NOT IN (SELECT DISTINCT doc_id FROM events)
		  AND id NOT IN (SELECT DISTINCT doc_id FROM snapshots)`,
		keepID,
	)
	if err != nil {
		return 0, fmt.Errorf("gc empty scratch: %w", err)
	}
	n, _ := res.RowsAffected() // best-effort count; deletion already committed
	return n, nil
}

// RecoverableScratch returns the IDs of GENUINE untitled scratch documents that
// carry history (events or snapshots) from a prior session, excluding excludeID
// and the chat sentinel (non-empty path). Newest first. These rows hold unsaved
// work the user can recover on the next launch.
//
// The `inode = 0` filter is load-bearing: a genuine scratch always has inode 0
// (CreateScratch inserts inode=0), whereas an orphaned BOUND document whose path
// was cleared by inode-change eviction RETAINS its real inode. Without this
// filter those zombie rows surface as fake "Untitled" tabs showing real-file
// content (a data-corruption-looking bug). Emptiness is filtered by the caller,
// which reconstructs each candidate and drops empty/whitespace-only content.
func (s *Store) RecoverableScratch(excludeID int64) ([]int64, error) {
	rows, err := s.perm.Query(`
		SELECT id FROM documents
		WHERE path='' AND id!=? AND (inode IS NULL OR inode = 0)
		  AND (id IN (SELECT DISTINCT doc_id FROM events)
		    OR id IN (SELECT DISTINCT doc_id FROM snapshots))
		ORDER BY id DESC`,
		excludeID,
	)
	if err != nil {
		return nil, fmt.Errorf("recoverable scratch: %w", err)
	}
	defer rows.Close()

	var ids []int64
	for rows.Next() {
		var id int64
		if err := rows.Scan(&id); err != nil {
			return nil, fmt.Errorf("recoverable scratch: scan: %w", err)
		}
		ids = append(ids, id)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("recoverable scratch: rows: %w", err)
	}
	return ids, nil
}
