package docstate

import (
	"bytes"
	"crypto/sha256"
	"database/sql"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"math"
	"time"

	"github.com/klauspost/compress/zstd"
	"rune/pkg/editor/buffer"
)

func (s *Store) PutBlob(content string) (string, error) {
	sum := sha256.Sum256([]byte(content))
	hash := hex.EncodeToString(sum[:])

	var buf bytes.Buffer
	enc, err := zstd.NewWriter(&buf)
	if err != nil {
		return "", fmt.Errorf("put blob: create zstd encoder: %w", err)
	}
	if _, err = enc.Write([]byte(content)); err != nil {
		return "", fmt.Errorf("put blob: compress: %w", err)
	}
	if err = enc.Close(); err != nil {
		return "", fmt.Errorf("put blob: finalize zstd: %w", err)
	}
	compressed := buf.Bytes()

	_, err = s.perm.Exec(
		`INSERT OR IGNORE INTO blobs(hash, content) VALUES(?,?)`,
		hash, compressed,
	)
	if err != nil {
		return "", fmt.Errorf("put blob %s: %w", hash, err)
	}
	return hash, nil
}

func (s *Store) GetBlob(hash string) (string, error) {
	var compressed []byte
	err := s.perm.QueryRow(`SELECT content FROM blobs WHERE hash=?`, hash).Scan(&compressed)
	if err != nil {
		return "", fmt.Errorf("get blob %s: %w", hash, err)
	}

	dec, err := zstd.NewReader(bytes.NewReader(compressed))
	if err != nil {
		return "", fmt.Errorf("get blob %s: create zstd decoder: %w", hash, err)
	}
	defer dec.Close()

	data, err := io.ReadAll(dec)
	if err != nil {
		return "", fmt.Errorf("get blob %s: decompress: %w", hash, err)
	}
	return string(data), nil
}

// CreateSnapshot stores a snapshot for docID at the current journal position
// (seq). seq should be the most recently returned seq from AppendEdit so that
// RecoverDocument can find this snapshot as the closest anchor for any replay.
func (s *Store) CreateSnapshot(docID int64, content, source string, seq int64) (int64, error) {
	hash, err := s.PutBlob(content)
	if err != nil {
		return 0, fmt.Errorf("create snapshot doc %d: %w", docID, err)
	}

	at := s.clock().UTC().Format(time.RFC3339Nano)
	res, err := s.perm.Exec(
		`INSERT INTO snapshots(doc_id, blob_hash, source, seq, created_at) VALUES(?,?,?,?,?)`,
		docID, hash, source, seq, at,
	)
	if err != nil {
		return 0, fmt.Errorf("create snapshot doc %d: %w", docID, err)
	}

	id, err := res.LastInsertId()
	if err != nil {
		return 0, fmt.Errorf("create snapshot doc %d: last insert id: %w", docID, err)
	}
	return id, nil
}

// RecoverDocument reconstructs the current content for docID by finding the
// most recent snapshot whose seq is ≤ the document's current undo position,
// then forward-replaying all edit events between the snapshot seq and the
// current position using buffer.ReplayForward.
//
// Algorithm:
//  1. Read current_seq from documents (NULL = at head → use MaxInt64).
//  2. Find newest snapshot with seq ≤ targetSeq; anchorContent = "" if none.
//  3. Gather edits from events with seq > anchorSnapshotSeq AND seq ≤ targetSeq.
//  4. Apply buffer.ReplayForward(anchorContent, batches) and return.
func (s *Store) RecoverDocument(docID int64) (string, error) {
	// Step 1: read current undo position.
	var nullableCS sql.NullInt64
	if err := s.perm.QueryRow(`SELECT current_seq FROM documents WHERE id=?`, docID).Scan(&nullableCS); err != nil {
		return "", fmt.Errorf("recover doc %d: read current_seq: %w", docID, err)
	}
	targetSeq := int64(math.MaxInt64)
	if nullableCS.Valid {
		targetSeq = nullableCS.Int64
	}

	// Step 2: find the nearest snapshot at or before targetSeq. Break ties by id
	// DESC (most-recently-written wins): coalesced edits keep the SAME journal seq
	// (AppendEdit updates the existing event in place), so several snapshots can
	// share one seq with progressively newer content. A seq-only `ORDER BY seq DESC`
	// picks an arbitrary one at the tie and can anchor on STALE content, dropping the
	// latest coalesced keystroke from recovery (a §1.4.3 data-loss). id DESC selects
	// the freshest snapshot at that seq, matching LatestSnapshot's ordering.
	var anchorSnapshotSeq int64
	var anchorContent string
	var blobHash string
	err := s.perm.QueryRow(
		`SELECT seq, blob_hash FROM snapshots WHERE doc_id=? AND seq <= ? ORDER BY seq DESC, id DESC LIMIT 1`,
		docID, targetSeq,
	).Scan(&anchorSnapshotSeq, &blobHash)
	if err != nil && err != sql.ErrNoRows {
		return "", fmt.Errorf("recover doc %d: find anchor snapshot: %w", docID, err)
	}
	if err == nil {
		anchorContent, err = s.GetBlob(blobHash)
		if err != nil {
			return "", fmt.Errorf("recover doc %d: get anchor blob: %w", docID, err)
		}
	}

	// Step 3: gather edit batches between the anchor and the target position.
	rows, err := s.perm.Query(
		`SELECT edits FROM events WHERE doc_id=? AND kind='edit' AND seq > ? AND seq <= ? ORDER BY seq ASC`,
		docID, anchorSnapshotSeq, targetSeq,
	)
	if err != nil {
		return "", fmt.Errorf("recover doc %d: query events: %w", docID, err)
	}
	defer rows.Close()

	var batches [][]buffer.AppliedEdit
	for rows.Next() {
		var editsJSON string
		if err := rows.Scan(&editsJSON); err != nil {
			return "", fmt.Errorf("recover doc %d: scan event edits: %w", docID, err)
		}
		var batch []buffer.AppliedEdit
		if err := json.Unmarshal([]byte(editsJSON), &batch); err != nil {
			return "", fmt.Errorf("recover doc %d: unmarshal edits: %w", docID, err)
		}
		batches = append(batches, batch)
	}
	if err := rows.Err(); err != nil {
		return "", fmt.Errorf("recover doc %d: events rows: %w", docID, err)
	}

	// Step 4: replay edits from the anchor snapshot forward.
	return buffer.ReplayForward(anchorContent, batches), nil
}

// Content returns the current content of docID by reconstructing it from the
// VFS (snapshot + replayed edits). This is what autosave reads; disk is only
// written on an explicit materialise (⌘S).
func (s *Store) Content(docID int64) (string, error) {
	return s.RecoverDocument(docID)
}

// ActiveEdits returns the surface's edit batches that are LIVE at the document's
// current undo position — i.e. with seq <= current_seq, honoring undo/redo (unlike
// AllEdits, which returns the whole log regardless of the undo head). Ordered by
// seq. Used by the fuzz mirror to reconstruct the live buffer as loaded-baseline +
// ReplayForward(ActiveEdits); a snapshot-anchored RecoverDocument cannot serve that
// because the loaded baseline is never snapshotted at genesis.
func (s *Store) ActiveEdits(docID int64, surface string) ([][]buffer.AppliedEdit, error) {
	var nullableCS sql.NullInt64
	if err := s.perm.QueryRow(`SELECT current_seq FROM documents WHERE id=?`, docID).Scan(&nullableCS); err != nil {
		return nil, fmt.Errorf("active edits doc %d: read current_seq: %w", docID, err)
	}
	targetSeq := int64(math.MaxInt64)
	if nullableCS.Valid {
		targetSeq = nullableCS.Int64
	}

	rows, err := s.perm.Query(
		`SELECT edits FROM events WHERE doc_id=? AND surface=? AND kind='edit' AND seq <= ? ORDER BY seq ASC`,
		docID, surface, targetSeq,
	)
	if err != nil {
		return nil, fmt.Errorf("active edits doc %d surface %q: %w", docID, surface, err)
	}
	defer rows.Close()

	var result [][]buffer.AppliedEdit
	for rows.Next() {
		var editsJSON string
		if err := rows.Scan(&editsJSON); err != nil {
			return nil, fmt.Errorf("active edits scan doc %d surface %q: %w", docID, surface, err)
		}
		var batch []buffer.AppliedEdit
		if err := json.Unmarshal([]byte(editsJSON), &batch); err != nil {
			return nil, fmt.Errorf("active edits unmarshal doc %d surface %q: %w", docID, surface, err)
		}
		result = append(result, batch)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("active edits rows doc %d surface %q: %w", docID, surface, err)
	}
	return result, nil
}

// HasHistory reports whether docID has any events or snapshots in the VFS.
// Use this to distinguish "no VFS record yet" (false → fall back to disk)
// from "VFS record exists" (true → use RecoverDocument even if content is
// empty, e.g. the user deleted all text and the deletion was journaled).
func (s *Store) HasHistory(docID int64) (bool, error) {
	var n int
	err := s.perm.QueryRow(
		`SELECT EXISTS(
			SELECT 1 FROM events WHERE doc_id=?
			UNION ALL
			SELECT 1 FROM snapshots WHERE doc_id=?
		)`,
		docID, docID,
	).Scan(&n)
	if err != nil {
		return false, fmt.Errorf("has history doc %d: %w", docID, err)
	}
	return n > 0, nil
}

// LatestSnapshot returns the raw content of the most recent snapshot for
// docID. Prefer RecoverDocument/Content in new code — this does not apply
// pending edits and is only useful for diagnostic / migration purposes.
func (s *Store) LatestSnapshot(docID int64) (string, error) {
	var hash string
	err := s.perm.QueryRow(
		`SELECT blob_hash FROM snapshots WHERE doc_id=? ORDER BY id DESC LIMIT 1`,
		docID,
	).Scan(&hash)
	if err == sql.ErrNoRows {
		return "", sql.ErrNoRows
	}
	if err != nil {
		return "", fmt.Errorf("latest snapshot doc %d: %w", docID, err)
	}

	content, err := s.GetBlob(hash)
	if err != nil {
		return "", fmt.Errorf("latest snapshot doc %d: %w", docID, err)
	}
	return content, nil
}
