package markdownedit

import (
	"testing"

	"rune/pkg/terminal"
	"rune/pkg/ui/components/textedit"
	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"
)

// newSearchModel builds a plain markdownedit model sized wide enough to render
// the test content, set unfocused (as it would be while the search bar is open).
func newSearchModel(t *testing.T, content string) Model {
	t.Helper()
	keys := keymap.Default()
	st := styles.Default()
	m := New(keys, st, terminal.TermCaps{})
	m = m.SetRect(textedit.Rect{W: 80, H: 24})
	m = m.SetContent(content)
	// Editor is unfocused while the search bar holds focus (SyncNoReveal).
	m = m.SetFocused(false)
	return m
}

// TestSearchVisibility_MatchInURLHidden verifies that a match that falls
// entirely inside the hidden URL portion of a markdown link — e.g. "(url)"
// in "[text](url)" — is excluded from the navigable match set.
//
// In SyncNoReveal mode the "(url)" bytes have no rendered cells, so the span
// cannot be scrolled-to or selected. filterVisibleMatches must drop it.
func TestSearchVisibility_MatchInURLHidden(t *testing.T) {
	// "[click](https://example.com)" — "example" is inside the hidden URL.
	m := newSearchModel(t, "[click](https://example.com)")
	m = m.SetSearchQuery("example", true)

	_, total := m.MatchCount()
	if total > 0 {
		t.Errorf("expected 0 navigable matches (URL hidden), got %d", total)
	}
}

// TestSearchVisibility_MatchInVisibleLinkText verifies that a match covering
// the visible text portion of a markdown link is navigable.
func TestSearchVisibility_MatchInVisibleLinkText(t *testing.T) {
	// "click" is the visible text — it should be navigable.
	m := newSearchModel(t, "[click here](https://example.com)")
	m = m.SetSearchQuery("click", true)

	_, total := m.MatchCount()
	if total == 0 {
		t.Errorf("expected ≥1 navigable match for visible link text, got 0")
	}
}

// TestSearchVisibility_MatchInsideBoldHidden verifies that a match that falls
// entirely inside the hidden "**" bold delimiters is excluded from the
// navigable match set (the asterisks have no rendered cells in SyncNoReveal).
func TestSearchVisibility_MatchInsideBoldHidden(t *testing.T) {
	// "**" delimiters are hidden in rendered mode; a literal search for "**"
	// should find 0 navigable matches.
	m := newSearchModel(t, "**bold text**")
	m = m.SetSearchQuery("**", true)

	_, total := m.MatchCount()
	if total > 0 {
		t.Errorf("expected 0 navigable matches for hidden ** delimiters, got %d", total)
	}
}

// TestSearchVisibility_MatchInBoldTextVisible verifies that a match covering
// the visible bold content is navigable.
func TestSearchVisibility_MatchInBoldTextVisible(t *testing.T) {
	m := newSearchModel(t, "**bold text**")
	m = m.SetSearchQuery("bold", true)

	_, total := m.MatchCount()
	if total == 0 {
		t.Errorf("expected ≥1 navigable match for visible bold text, got 0")
	}
}

// TestSearchVisibility_PlainTextAlwaysVisible verifies that a plain-text match
// (no markdown syntax) is always navigable in both focused and unfocused modes.
func TestSearchVisibility_PlainTextAlwaysVisible(t *testing.T) {
	m := newSearchModel(t, "hello world, hello again")
	m = m.SetSearchQuery("hello", true)

	_, total := m.MatchCount()
	if total != 2 {
		t.Errorf("expected 2 navigable matches, got %d", total)
	}
}
