package docstate

import (
	"bytes"
	"crypto/sha256"
	"database/sql"
	"encoding/hex"
	"fmt"
	"io"
	"time"

	"github.com/klauspost/compress/zstd"
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

func (s *Store) CreateSnapshot(docID int64, content, source string) (int64, error) {
	hash, err := s.PutBlob(content)
	if err != nil {
		return 0, fmt.Errorf("create snapshot doc %d: %w", docID, err)
	}

	at := s.clock().UTC().Format(time.RFC3339Nano)
	res, err := s.perm.Exec(
		`INSERT INTO snapshots(doc_id, blob_hash, parent_ids, source, created_at) VALUES(?,?,NULL,?,?)`,
		docID, hash, source, at,
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
