package textedit

import (
	"testing"

	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"
)

// TestSetContentInvalidatesStaleSearchMatches is the regression test for a bug
// where SetContent — the chokepoint used whenever the displayed document is
// swapped wholesale (file load, Help toggle, Untitled reset) — never bumped
// Model.rev, so recomputeIfStale never re-ran the active search query against
// the new buffer. FindNext then applied the OLD document's match byte offsets
// to the NEW document's content: in range (no crash, no clamp), but pointing
// at unrelated, wrong text.
//
// D5: package textedit (internal), not textedit_test — reads the
// searchMatches/searchActive fields directly since their former exported
// accessors (SearchMatches/SearchActive) had no production caller and were
// unexported (deleted; the field read replaces them here).
func TestSetContentInvalidatesStaleSearchMatches(t *testing.T) {
	m := New(keymap.Default(), styles.Default())
	m = m.SetRect(Rect{W: 80, H: 24})
	m = m.SetContent("hello world hello")
	m = m.SetSearchQuery("hello", false)

	if _, total := m.MatchCount(); total != 2 {
		t.Fatalf("MatchCount() before switch: total = %d, want 2", total)
	}

	// Switch to an unrelated document. "hello" appears once here, at byte
	// offset 6 — different from either match's offset in the old document
	// (0 and 12), so a stale offset would land on the wrong text.
	m = m.SetContent("xxxxx hello yyyyy")
	m = m.FindNext()

	if _, total := m.MatchCount(); total != 1 {
		t.Fatalf("MatchCount() after switch: total = %d, want 1 (stale matches from the old document leaked through)", total)
	}
	matches := m.searchMatches
	if len(matches) != 1 || matches[0].Start != 6 || matches[0].End != 11 {
		t.Fatalf("searchMatches = %+v, want a single match at [6,11) in the new document", matches)
	}
}

// TestSearchActive_ValidityOutOfBand is the regression test for §1.7: no
// active match must be represented by ActiveMatch.Valid==false, not a -1
// sentinel folded into Index. Also guards the exact behaviors the sentinel
// removal must preserve: nearest-match selection, wrap-around on repeated
// FindNext/FindPrev, and 1-based MatchCount.
func TestSearchActive_ValidityOutOfBand(t *testing.T) {
	m := New(keymap.Default(), styles.Default())
	m = m.SetRect(Rect{W: 80, H: 24})
	m = m.SetContent("hello world hello there hello")

	// No query yet: no active match.
	if am := m.searchActive; am.Valid {
		t.Fatalf("searchActive before any query: got Valid=true, want false (am=%+v)", am)
	}

	m = m.SetSearchQuery("hello", false)
	if am := m.searchActive; am.Valid {
		t.Fatalf("searchActive right after SetSearchQuery: got Valid=true, want false (am=%+v)", am)
	}
	if idx, total := m.MatchCount(); idx != 0 || total != 3 {
		t.Fatalf("MatchCount() before any navigation: got idx=%d total=%d, want idx=0 total=3", idx, total)
	}

	// FindNext from cursor 0 picks the nearest match at/after the cursor —
	// the first "hello" at offset 0.
	m = m.FindNext()
	am := m.searchActive
	if !am.Valid || am.Index != 0 {
		t.Fatalf("FindNext (nearest-match): got %+v, want {Index:0 Valid:true}", am)
	}
	if idx, total := m.MatchCount(); idx != 1 || total != 3 {
		t.Fatalf("MatchCount() after first FindNext: got idx=%d total=%d, want idx=1 total=3 (1-based)", idx, total)
	}

	// Advance through all matches and confirm wrap-around back to the first.
	m = m.FindNext() // -> match 1
	m = m.FindNext() // -> match 2
	am = m.searchActive
	if !am.Valid || am.Index != 2 {
		t.Fatalf("FindNext x3: got %+v, want {Index:2 Valid:true}", am)
	}
	m = m.FindNext() // wrap -> match 0
	am = m.searchActive
	if !am.Valid || am.Index != 0 {
		t.Fatalf("FindNext wrap-around: got %+v, want {Index:0 Valid:true}", am)
	}

	// FindPrev wraps the other direction.
	m = m.FindPrev()
	am = m.searchActive
	if !am.Valid || am.Index != 2 {
		t.Fatalf("FindPrev wrap-around: got %+v, want {Index:2 Valid:true}", am)
	}

	// ClearSearch must return to the no-active-match state.
	m = m.ClearSearch()
	if am := m.searchActive; am.Valid {
		t.Fatalf("searchActive after ClearSearch: got Valid=true, want false (am=%+v)", am)
	}
	if idx, total := m.MatchCount(); idx != 0 || total != 0 {
		t.Fatalf("MatchCount() after ClearSearch: got idx=%d total=%d, want idx=0 total=0", idx, total)
	}
}
