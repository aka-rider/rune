package docstate

import (
	"testing"
	"time"
)

func TestSearchHistory_AppendAndRetrieve(t *testing.T) {
	// A strictly ticking clock, not time.Now: SearchHistory orders by
	// last_used_at DESC, and two appends inside the same clock tick would
	// tie — a pre-existing flake this ordering assertion tripped on.
	var tick int64
	clock := func() time.Time { tick++; return time.Unix(tick, 0) }
	s, err := OpenInMemory(clock)
	if err != nil {
		t.Fatalf("OpenInMemory: %v", err)
	}
	defer s.Close()

	if err := s.AppendSearchQuery("first"); err != nil {
		t.Fatalf("AppendSearchQuery: %v", err)
	}
	if err := s.AppendSearchQuery("second"); err != nil {
		t.Fatalf("AppendSearchQuery: %v", err)
	}

	entries, err := s.SearchHistory()
	if err != nil {
		t.Fatalf("SearchHistory: %v", err)
	}
	if len(entries) != 2 {
		t.Fatalf("got %d entries, want 2", len(entries))
	}
	// most-recent first
	if entries[0] != "second" || entries[1] != "first" {
		t.Errorf("order wrong: %v", entries)
	}
}

func TestSearchHistory_EmptyIgnored(t *testing.T) {
	s, err := OpenInMemory(func() time.Time { return time.Now() })
	if err != nil {
		t.Fatalf("OpenInMemory: %v", err)
	}
	defer s.Close()

	if err := s.AppendSearchQuery(""); err != nil {
		t.Fatalf("AppendSearchQuery empty: %v", err)
	}
	if err := s.AppendSearchQuery("   "); err != nil {
		t.Fatalf("AppendSearchQuery whitespace: %v", err)
	}

	entries, err := s.SearchHistory()
	if err != nil {
		t.Fatalf("SearchHistory: %v", err)
	}
	if len(entries) != 0 {
		t.Errorf("expected no entries, got %v", entries)
	}
}

func TestSearchHistory_DedupBumpsRecency(t *testing.T) {
	var tick int64
	clock := func() time.Time { tick++; return time.Unix(tick, 0) }
	s, err := OpenInMemory(clock)
	if err != nil {
		t.Fatalf("OpenInMemory: %v", err)
	}
	defer s.Close()

	if err := s.AppendSearchQuery("alpha"); err != nil {
		t.Fatal(err)
	}
	if err := s.AppendSearchQuery("beta"); err != nil {
		t.Fatal(err)
	}
	// Re-insert "alpha" — should bump its recency to be more recent than "beta".
	if err := s.AppendSearchQuery("alpha"); err != nil {
		t.Fatal(err)
	}

	entries, err := s.SearchHistory()
	if err != nil {
		t.Fatalf("SearchHistory: %v", err)
	}
	if len(entries) != 2 {
		t.Fatalf("expected 2 entries (dedup), got %d", len(entries))
	}
	if entries[0] != "alpha" {
		t.Errorf("most-recent should be alpha, got %q", entries[0])
	}
}

func TestSearchHistory_Prune(t *testing.T) {
	s, err := OpenInMemory(func() time.Time { return time.Now() })
	if err != nil {
		t.Fatalf("OpenInMemory: %v", err)
	}
	defer s.Close()

	// Insert more than the cap.
	for i := 0; i < searchHistoryMax+10; i++ {
		q := string(rune('a'+i%26)) + string(rune('0'+i/26))
		if err := s.AppendSearchQuery(q); err != nil {
			t.Fatalf("AppendSearchQuery %d: %v", i, err)
		}
	}

	entries, err := s.SearchHistory()
	if err != nil {
		t.Fatalf("SearchHistory: %v", err)
	}
	if len(entries) > searchHistoryMax {
		t.Errorf("expected ≤%d entries, got %d", searchHistoryMax, len(entries))
	}
}
