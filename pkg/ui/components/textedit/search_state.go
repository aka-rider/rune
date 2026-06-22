package textedit

import (
	"rune/pkg/editor/cursor"
	"rune/pkg/editor/display"
	"rune/pkg/editor/search"
)

// SetSearchQuery updates the set of search matches for the given query.
// It recomputes matches against the current buffer and filters to only those
// that cover at least one rendered cell (so hidden markdown syntax is excluded).
// No scroll or selection change; active match index is reset to -1.
func (m Model) SetSearchQuery(query string, caseInsensitive bool) Model {
	m.searchQuery = query
	m.searchCaseInsensitive = caseInsensitive

	if query == "" {
		m.searchMatches = nil
		m.searchActive = -1
		m.searchRev = m.rev
		return m
	}

	rawMatches := search.Find(m.buf.Content(), query, caseInsensitive)
	m.searchMatches = m.filterVisibleMatches(rawMatches)
	m.searchActive = -1
	m.searchRev = m.rev
	return m
}

// filterVisibleMatches keeps only spans that cover at least one rendered cell.
// Spans entirely inside hidden markdown regions (e.g. **, [url], etc.) produce
// no cells in rendered mode and are therefore not navigable.
func (m Model) filterVisibleMatches(spans []search.Span) []SelInterval {
	if len(spans) == 0 {
		return nil
	}

	// Build a set of byte offsets that map to visible cells by scanning
	// all display lines in the snapshot.
	visible := make(map[int]bool)
	allLines := m.snapshot.Slice(0, m.snapshot.TotalRows)
	for _, l := range allLines {
		for _, sp := range l.Spans {
			if sp.State == display.Revealed {
				// All bytes in [BufferStart, BufferEnd) are visible (excluding newlines).
				for off := sp.BufferStart; off < sp.BufferEnd; off++ {
					visible[off] = true
				}
			} else {
				// Rendered: only CellMap entries with a valid BufOffset are visible.
				for _, cm := range sp.CellMap {
					if cm.BufOffset >= 0 {
						visible[cm.BufOffset] = true
					}
				}
			}
		}
	}

	var result []SelInterval
	for _, sp := range spans {
		for off := sp.Start; off < sp.End; off++ {
			if visible[off] {
				result = append(result, SelInterval{sp.Start, sp.End})
				break
			}
		}
	}
	return result
}

// recomputeIfStale re-runs SetSearchQuery if the buffer has changed since the
// last computation. Called before any navigation to avoid stale offset jumps.
func (m Model) recomputeIfStale() Model {
	if m.rev != m.searchRev && m.searchQuery != "" {
		m = m.SetSearchQuery(m.searchQuery, m.searchCaseInsensitive)
	}
	return m
}

// FindNext moves to the next search match after the current cursor position,
// wrapping around. Sets the cursor selection to the match and scrolls it into
// view. No-op when there are no matches.
func (m Model) FindNext() Model {
	m = m.recomputeIfStale()
	if len(m.searchMatches) == 0 {
		return m
	}

	if m.searchActive == -1 {
		// Pick nearest match at or after the primary cursor.
		cursorOff := m.cursors.Primary().Position
		found := false
		for i, sm := range m.searchMatches {
			if sm.Start >= cursorOff {
				m.searchActive = i
				found = true
				break
			}
		}
		if !found {
			// Wrap: use first match.
			m.searchActive = 0
		}
	} else {
		m.searchActive = (m.searchActive + 1) % len(m.searchMatches)
	}

	return m.selectActiveMatch()
}

// FindPrev moves to the previous search match before the current cursor
// position, wrapping around. Sets the cursor selection to the match and scrolls
// it into view. No-op when there are no matches.
func (m Model) FindPrev() Model {
	m = m.recomputeIfStale()
	if len(m.searchMatches) == 0 {
		return m
	}

	if m.searchActive == -1 {
		// Pick nearest match at or before the primary cursor.
		cursorOff := m.cursors.Primary().Position
		found := false
		for i := len(m.searchMatches) - 1; i >= 0; i-- {
			if m.searchMatches[i].End <= cursorOff {
				m.searchActive = i
				found = true
				break
			}
		}
		if !found {
			// Wrap: use last match.
			m.searchActive = len(m.searchMatches) - 1
		}
	} else {
		m.searchActive = (m.searchActive - 1 + len(m.searchMatches)) % len(m.searchMatches)
	}

	return m.selectActiveMatch()
}

// selectActiveMatch places the cursor at the active match and scrolls it into view.
func (m Model) selectActiveMatch() Model {
	if m.searchActive < 0 || m.searchActive >= len(m.searchMatches) {
		return m
	}
	active := m.searchMatches[m.searchActive]
	// Clamp to live buffer length to guard against stale spans.
	bufLen := m.buf.Len()
	start := active.Start
	end := active.End
	if start > bufLen {
		start = bufLen
	}
	if end > bufLen {
		end = bufLen
	}
	primary := cursor.Cursor{
		Position: end,
		Anchor:   start,
		ID:       1,
	}
	m.cursors = cursor.NewCursorSetFrom([]cursor.Cursor{primary})
	m = m.ScrollToCursor()
	return m
}

// ClearSearch removes all search match state, leaving any current selection
// intact so the user ends up with the last navigated match selected on close.
func (m Model) ClearSearch() Model {
	m.searchMatches = nil
	m.searchActive = -1
	m.searchQuery = ""
	return m
}

// SearchMatches returns the current slice of match intervals.
func (m Model) SearchMatches() []SelInterval { return m.searchMatches }

// SearchActive returns the index of the currently-active match, or -1.
func (m Model) SearchActive() int { return m.searchActive }

// MatchCount returns the 1-based index of the active match and total count.
// idx is 0 when no match is active.
func (m Model) MatchCount() (idx, total int) {
	total = len(m.searchMatches)
	if m.searchActive >= 0 {
		idx = m.searchActive + 1
	}
	return
}

