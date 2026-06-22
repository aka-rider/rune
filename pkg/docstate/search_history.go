package docstate

import (
	"fmt"
	"strings"
	"time"
)

const searchHistoryMax = 200

// AppendSearchQuery records a search query in the global search history.
// Empty or whitespace-only queries are silently ignored.
// Re-submitting an existing query bumps its recency (upsert). The table is
// pruned to the most-recent searchHistoryMax entries after each write.
func (s *Store) AppendSearchQuery(query string) error {
	if strings.TrimSpace(query) == "" {
		return nil
	}
	at := s.clock().UTC().Format(time.RFC3339Nano)
	_, err := s.perm.Exec(
		`INSERT INTO search_history(query, last_used_at) VALUES(?,?)
		ON CONFLICT(query) DO UPDATE SET last_used_at=excluded.last_used_at`,
		query, at,
	)
	if err != nil {
		return fmt.Errorf("append search query: %w", err)
	}
	// Prune to most-recent searchHistoryMax entries.
	_, err = s.perm.Exec(
		`DELETE FROM search_history WHERE query NOT IN (
			SELECT query FROM search_history ORDER BY last_used_at DESC LIMIT ?
		)`,
		searchHistoryMax,
	)
	if err != nil {
		return fmt.Errorf("prune search history: %w", err)
	}
	return nil
}

// SearchHistory returns recent search queries, most-recent first, capped at
// searchHistoryMax entries.
func (s *Store) SearchHistory() ([]string, error) {
	rows, err := s.perm.Query(
		`SELECT query FROM search_history ORDER BY last_used_at DESC LIMIT ?`,
		searchHistoryMax,
	)
	if err != nil {
		return nil, fmt.Errorf("search history: %w", err)
	}
	defer rows.Close()

	var queries []string
	for rows.Next() {
		var q string
		if err := rows.Scan(&q); err != nil {
			return nil, fmt.Errorf("search history scan: %w", err)
		}
		queries = append(queries, q)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("search history rows: %w", err)
	}
	return queries, nil
}
