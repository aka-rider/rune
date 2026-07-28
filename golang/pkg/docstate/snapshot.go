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

// GetBlob decompresses and returns the content stored under hash, verifying
// its SHA-256 against hash before returning (blob rot / bit-flip detection —
// closes the "GetBlob never re-verifies SHA-256" structural gap). A mismatch
// is a corrupt blob and is surfaced as an error, never silently returned.
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

	sum := sha256.Sum256(data)
	if got := hex.EncodeToString(sum[:]); got != hash {
		return "", fmt.Errorf("get blob %s: content hash mismatch (corrupt blob): got %s", hash, got)
	}
	return string(data), nil
}

// CreateSnapshot stores a PURE recovery anchor for docID at the current
// journal position (seq), tagged with THIS Store's own session_id (v10) —
// a snapshot anchors ONE session's own replay window; two sessions editing
// the same docID keep entirely separate anchor chains. seq should be the
// most recently returned seq from AppendEdit so that RecoverDocument can
// find this snapshot as the closest anchor for any replay. No source
// taxonomy (Part III) — the disk fact and the 3-way-merge ancestor are
// served entirely by observations/saved_obs/ancestorAt; a snapshot's only
// job is bounding how far RecoverDocument ever has to replay.
func (s *Store) CreateSnapshot(docID int64, content string, seq int64) (int64, error) {
	hash, err := s.PutBlob(content)
	if err != nil {
		return 0, fmt.Errorf("create snapshot doc %d: %w", docID, err)
	}

	at := s.clock().UTC().Format(time.RFC3339Nano)
	res, err := s.perm.Exec(
		`INSERT INTO snapshots(doc_id, session_id, blob_hash, seq, created_at) VALUES(?,?,?,?,?)`,
		docID, s.sessionID, hash, seq, at,
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

// recoverAt reconstructs docID's content AS SEEN BY sessionID specifically
// (v10) — the session-scoped engine behind both RecoverDocument (always
// s.sessionID) and RecoverAcrossSessions/Load's cross-session read of an
// already-identified OTHER (confirmed-dead) session's content. Finds the
// most recent snapshot tagged with sessionID whose seq is ≤ that session's
// current undo position, then forward-replays only THAT session's own edit
// events between the snapshot seq and the current position using
// buffer.ReplayForward.
//
// Algorithm:
//  1. Read sessionID's own current_seq from session_documents (no row, or
//     NULL = at head → use MaxInt64).
//  2. Find newest snapshot tagged sessionID with seq ≤ targetSeq;
//     anchorContent = "" if none.
//  3. Gather sessionID's own edits from events with seq > anchorSnapshotSeq
//     AND seq ≤ targetSeq.
//  4. Apply buffer.ReplayForward(anchorContent, batches) and return.
func (s *Store) recoverAt(docID, sessionID int64) (string, error) {
	// Step 1: read sessionID's own current undo position.
	var nullableCS sql.NullInt64
	if err := s.perm.QueryRow(`SELECT current_seq FROM session_documents WHERE session_id=? AND doc_id=?`, sessionID, docID).Scan(&nullableCS); err != nil && err != sql.ErrNoRows {
		return "", fmt.Errorf("recover doc %d session %d: read current_seq: %w", docID, sessionID, err)
	}
	targetSeq := int64(math.MaxInt64)
	if nullableCS.Valid {
		targetSeq = nullableCS.Int64
	}

	// Step 2: find the nearest snapshot, TAGGED WITH sessionID, at or before
	// targetSeq. Break ties by id DESC (most-recently-written wins):
	// coalesced edits keep the SAME journal seq (AppendEdit updates the
	// existing event in place), so several snapshots can share one seq with
	// progressively newer content. A seq-only `ORDER BY seq DESC` picks an
	// arbitrary one at the tie and can anchor on STALE content, dropping the
	// latest coalesced keystroke from recovery (a §1.4.3 data-loss). id DESC
	// selects the freshest snapshot at that seq — the tie-break policy this
	// query alone implements (D7: the doc comment used to cite a
	// `LatestSnapshot` helper that no longer exists in this package).
	var anchorSnapshotSeq int64
	var anchorContent string
	var blobHash string
	err := s.perm.QueryRow(
		`SELECT seq, blob_hash FROM snapshots WHERE doc_id=? AND session_id=? AND seq <= ? ORDER BY seq DESC, id DESC LIMIT 1`,
		docID, sessionID, targetSeq,
	).Scan(&anchorSnapshotSeq, &blobHash)
	if err != nil && err != sql.ErrNoRows {
		return "", fmt.Errorf("recover doc %d session %d: find anchor snapshot: %w", docID, sessionID, err)
	}
	if err == nil {
		anchorContent, err = s.GetBlob(blobHash)
		if err != nil {
			return "", fmt.Errorf("recover doc %d session %d: get anchor blob: %w", docID, sessionID, err)
		}
	}

	// Step 3: gather sessionID's OWN edit batches between the anchor and the
	// target position — never a different session's, even for the same doc.
	batches, err := s.readEditBatches(
		`SELECT edits FROM events WHERE doc_id=? AND session_id=? AND seq > ? AND seq <= ? ORDER BY seq ASC`,
		docID, sessionID, anchorSnapshotSeq, targetSeq,
	)
	if err != nil {
		return "", fmt.Errorf("recover doc %d session %d: %w", docID, sessionID, err)
	}

	// Step 4: replay edits from the anchor snapshot forward.
	return buffer.ReplayForward(anchorContent, batches), nil
}

// RecoverDocument reconstructs the current content for docID AS SEEN BY
// THIS SESSION (v10 — recoverAt, above). Two sessions editing the same
// docID each reconstruct only their own journal; neither can ever see the
// other's unsaved edits through this call.
func (s *Store) RecoverDocument(docID int64) (string, error) {
	return s.recoverAt(docID, s.sessionID)
}

// Content returns the current content of docID by reconstructing it from the
// VFS (snapshot + replayed edits). This is what autosave reads; disk is only
// written on an explicit materialise (⌘S).
func (s *Store) Content(docID int64) (string, error) {
	return s.RecoverDocument(docID)
}

// EditRow pairs one journaled edit batch with the seq it was recorded at.
type EditRow struct {
	Seq   int64
	Edits []buffer.AppliedEdit
}

// EditsInRange returns docID's own edit rows with seq in (fromSeq, toSeq],
// each tagged with its seq, ordered ascending — the same anchored-window
// shape recoverAt uses for a snapshot anchor (step 3), generalized to a
// caller-supplied bound instead of a snapshot's seq, and exposing per-row
// seq so a caller can identify the CURRENT TAIL row (the one at seq ==
// toSeq) — the only row AppendEdit's keystroke-coalescing UPDATE can still
// mutate in place (coalescing only ever targets the doc's current max-seq
// row) — from every row strictly before it, which stays valid to cache as
// long as the caller re-verifies CurrentSeq every call and evicts on any
// decrease: a strictly-older row can still be deleted later by undo-then-
// edit truncation, but that always requires an earlier, separate settle
// that lowers CurrentSeq below it first (undo and append never land in the
// same Update — see the fuzz driver's mirrorFor, the caller this exists
// for). Session-scoped (v10): each fuzzed Store/workspace pair is its own
// session, so this only ever sees its own edits, never a different
// session's sharing the same docID.
func (s *Store) EditsInRange(docID, fromSeq, toSeq int64) ([]EditRow, error) {
	rows, err := s.perm.Query(
		`SELECT seq, edits FROM events WHERE doc_id=? AND session_id=? AND seq > ? AND seq <= ? ORDER BY seq ASC`,
		docID, s.sessionID, fromSeq, toSeq,
	)
	if err != nil {
		return nil, fmt.Errorf("edits in range doc %d: %w", docID, err)
	}
	defer rows.Close()

	var result []EditRow
	for rows.Next() {
		var seq int64
		var editsJSON string
		if err := rows.Scan(&seq, &editsJSON); err != nil {
			return nil, fmt.Errorf("edits in range doc %d: scan: %w", docID, err)
		}
		var batch []buffer.AppliedEdit
		if err := json.Unmarshal([]byte(editsJSON), &batch); err != nil {
			return nil, fmt.Errorf("edits in range doc %d: unmarshal: %w", docID, err)
		}
		result = append(result, EditRow{Seq: seq, Edits: batch})
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("edits in range doc %d: rows: %w", docID, err)
	}
	return result, nil
}

// HasHistory reports whether docID has any events or snapshots RECORDED BY
// THIS SESSION (v10). Use this to distinguish "this session has no VFS
// record yet" (false → fall back to disk, or to a DIFFERENT session's
// history via RecoverAcrossSessions/Load's cross-session inheritance) from
// "this session's VFS record exists" (true → use RecoverDocument even if
// content is empty, e.g. the user deleted all text and the deletion was
// journaled). A docID with lots of history under OTHER sessions still
// reports false here for a session that has never itself touched it.
func (s *Store) HasHistory(docID int64) (bool, error) {
	var n int
	err := s.perm.QueryRow(
		`SELECT EXISTS(
			SELECT 1 FROM events WHERE doc_id=? AND session_id=?
			UNION ALL
			SELECT 1 FROM snapshots WHERE doc_id=? AND session_id=?
		)`,
		docID, s.sessionID, docID, s.sessionID,
	).Scan(&n)
	if err != nil {
		return false, fmt.Errorf("has history doc %d: %w", docID, err)
	}
	return n > 0, nil
}
