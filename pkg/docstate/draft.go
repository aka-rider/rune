package docstate

import (
	"database/sql"
	"fmt"
	"time"
)

func (s *Store) UpsertDraft(surface, content string) error {
	at := s.clock().UTC().Format(time.RFC3339Nano)
	_, err := s.perm.Exec(
		`INSERT INTO drafts(surface, content, updated_at) VALUES(?,?,?)
		ON CONFLICT(surface) DO UPDATE SET content=excluded.content, updated_at=excluded.updated_at`,
		surface, content, at,
	)
	if err != nil {
		return fmt.Errorf("upsert draft %q: %w", surface, err)
	}
	return nil
}

func (s *Store) GetDraft(surface string) (string, error) {
	var content string
	err := s.perm.QueryRow(`SELECT content FROM drafts WHERE surface=?`, surface).Scan(&content)
	if err == sql.ErrNoRows {
		return "", nil
	}
	if err != nil {
		return "", fmt.Errorf("get draft %q: %w", surface, err)
	}
	return content, nil
}
