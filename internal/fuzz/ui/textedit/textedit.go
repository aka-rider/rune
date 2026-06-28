//go:build fuzzing

// Package textedit contains invariant checkers for the textedit/markdownedit
// component: cell layout (R1–R9), cursor geometry (C1–C3), presence (M1–M2),
// buffer line count (B1), selection coverage (S1), and the buffer-version
// monotonicity transition (B2).
package textedit

import (
	"fmt"
	"strings"

	"rune/internal/fuzz/invariant"
	"rune/internal/fuzz/snapshot"
)

// Check runs all L0 textedit/cursor/buffer invariants against s.
// Returns the first violation, or nil.
func Check(s snapshot.Snapshot) *invariant.Violation {
	contentLen := len(s.Content)

	// R1: cursor-cell count == active cursor count (only when editor is focused and
	// has at least one visible row — height==0 renders nothing, so no cursor cells exist)
	if s.Focused && s.CursorOffsets != nil && len(s.Cells) > 0 {
		activeCursorCount := len(s.CursorOffsets)
		cursorCellCount := 0
		for _, line := range s.Cells {
			for _, c := range line {
				if c.Cursor {
					cursorCellCount++
				}
			}
		}
		if cursorCellCount != activeCursorCount {
			// Build cursor offset list and per-line cell BufOffset summary for diagnosis.
			return &invariant.Violation{
				InvariantID: "R1",
				Message: fmt.Sprintf(
					"cursor-cell count %d != active cursor count %d",
					cursorCellCount, activeCursorCount,
				),
			}
		}
	}

	// R2: every offset in CursorOffsets must appear as Cursor=true in at least one cell.
	if s.Focused && s.CursorOffsets != nil && len(s.Cells) > 0 {
		seen := make(map[int]bool, len(s.CursorOffsets))
		for _, line := range s.Cells {
			for _, c := range line {
				if c.Cursor {
					seen[c.BufOffset] = true
				}
			}
		}
		for off := range s.CursorOffsets {
			if !seen[off] {
				return &invariant.Violation{
					InvariantID: "R2",
					Message:     fmt.Sprintf("cursor offset %d has no matching Cursor=true cell", off),
				}
			}
		}
	}

	// R3: within each line, non-negative BufOffsets must be non-decreasing.
	for lineIdx, line := range s.Cells {
		prev := -1
		for colIdx, c := range line {
			if c.BufOffset < 0 {
				continue
			}
			if c.BufOffset < prev {
				return &invariant.Violation{
					InvariantID: "R3",
					Message: fmt.Sprintf(
						"line %d col %d: BufOffset %d < previous %d (non-monotone)",
						lineIdx, colIdx, c.BufOffset, prev,
					),
				}
			}
			prev = c.BufOffset
		}
	}

	// R4: cells with BufOffset >= 0 must have Width >= 1.
	for lineIdx, line := range s.Cells {
		for colIdx, c := range line {
			if c.BufOffset >= 0 && c.Width < 1 {
				return &invariant.Violation{
					InvariantID: "R4",
					Message: fmt.Sprintf(
						"line %d col %d: BufOffset %d has Width %d (want >= 1)",
						lineIdx, colIdx, c.BufOffset, c.Width,
					),
				}
			}
		}
	}

	// R5: every cell BufOffset must be in [-1, len(Content)]; other negatives are invalid.
	for lineIdx, line := range s.Cells {
		for colIdx, c := range line {
			if c.BufOffset < -1 || c.BufOffset > contentLen {
				return &invariant.Violation{
					InvariantID: "R5",
					Message: fmt.Sprintf(
						"line %d col %d: BufOffset %d out of range [-1, %d]",
						lineIdx, colIdx, c.BufOffset, contentLen,
					),
				}
			}
		}
	}

	// R6: no two cells with the same BufOffset >= 0 should both have Cursor=true.
	if s.Focused && s.CursorOffsets != nil {
		cursorCells := make(map[int]int) // offset -> count of Cursor=true cells
		for _, line := range s.Cells {
			for _, c := range line {
				if c.Cursor && c.BufOffset >= 0 {
					cursorCells[c.BufOffset]++
				}
			}
		}
		for off, count := range cursorCells {
			if count > 1 {
				return &invariant.Violation{
					InvariantID: "R6",
					Message: fmt.Sprintf(
						"BufOffset %d has %d Cursor=true cells (want <= 1)",
						off, count,
					),
				}
			}
		}
	}

	// R7: every Cursor=true cell with BufOffset >= 0 must have its offset in CursorOffsets.
	if s.Focused && s.CursorOffsets != nil {
		for lineIdx, line := range s.Cells {
			for colIdx, c := range line {
				if c.Cursor && c.BufOffset >= 0 && !s.CursorOffsets[c.BufOffset] {
					return &invariant.Violation{
						InvariantID: "R7",
						Message: fmt.Sprintf(
							"line %d col %d: Cursor=true at BufOffset %d not in CursorOffsets",
							lineIdx, colIdx, c.BufOffset,
						),
					}
				}
			}
		}
	}

	// R8: no cell may carry a newline or carriage-return rune.
	for lineIdx, line := range s.Cells {
		for colIdx, c := range line {
			if c.Rune == '\n' || c.Rune == '\r' {
				return &invariant.Violation{
					InvariantID: "R8",
					Message: fmt.Sprintf(
						"line %d col %d: cell has control rune %q (must not appear in rendered cells)",
						lineIdx, colIdx, c.Rune,
					),
				}
			}
		}
	}

	// R9: decorative cells (BufOffset==-1) must never be marked Selected or Cursor.
	for lineIdx, line := range s.Cells {
		for colIdx, c := range line {
			if c.BufOffset == -1 && (c.Selected || c.Cursor) {
				return &invariant.Violation{
					InvariantID: "R9",
					Message: fmt.Sprintf(
						"line %d col %d: decorative cell (BufOffset==-1) is Selected=%v Cursor=%v",
						lineIdx, colIdx, c.Selected, c.Cursor,
					),
				}
			}
		}
	}

	// M1: when focused and cells are non-empty, at least one cursor offset must exist.
	if s.Focused && len(s.Cells) > 0 && len(s.CursorOffsets) < 1 {
		return &invariant.Violation{
			InvariantID: "M1",
			Message:     "editor focused with non-empty cells but CursorOffsets is empty",
		}
	}

	// M2: all offsets in CursorOffsets must be in [0, len(Content)] (EOF position valid).
	for off := range s.CursorOffsets {
		if off < 0 || off > contentLen {
			return &invariant.Violation{
				InvariantID: "M2",
				Message: fmt.Sprintf(
					"CursorOffset %d out of range [0, %d]",
					off, contentLen,
				),
			}
		}
	}

	// C1: cursors sorted by SelectionStart, non-overlapping.
	for i := 1; i < len(s.Cursors); i++ {
		prev := s.Cursors[i-1]
		cur := s.Cursors[i]
		if prev.SelectionEnd() > cur.SelectionStart() {
			return &invariant.Violation{
				InvariantID: "C1",
				Message: fmt.Sprintf(
					"cursor[%d] SelectionEnd %d > cursor[%d] SelectionStart %d (overlap/order)",
					i-1, prev.SelectionEnd(), i, cur.SelectionStart(),
				),
			}
		}
	}

	// C2: cursor IDs must be unique and positive.
	{
		seen := make(map[int]int, len(s.Cursors))
		for i, c := range s.Cursors {
			if c.ID <= 0 {
				return &invariant.Violation{
					InvariantID: "C2",
					Message:     fmt.Sprintf("cursor[%d] has non-positive ID %d", i, c.ID),
				}
			}
			if j, dup := seen[c.ID]; dup {
				return &invariant.Violation{
					InvariantID: "C2",
					Message:     fmt.Sprintf("cursor ID %d appears at indices %d and %d", c.ID, j, i),
				}
			}
			seen[c.ID] = i
		}
	}

	// C3: both Position and Anchor of each cursor must be in [0, len(Content)].
	for i, c := range s.Cursors {
		if c.Position < 0 || c.Position > contentLen {
			return &invariant.Violation{
				InvariantID: "C3",
				Message: fmt.Sprintf(
					"cursor[%d] Position %d out of [0, %d]", i, c.Position, contentLen,
				),
			}
		}
		if c.Anchor < 0 || c.Anchor > contentLen {
			return &invariant.Violation{
				InvariantID: "C3",
				Message: fmt.Sprintf(
					"cursor[%d] Anchor %d out of [0, %d]", i, c.Anchor, contentLen,
				),
			}
		}
	}

	// B1: buffer line count == strings.Count(Content, "\n") + 1.
	if s.LineCount > 0 {
		expected := strings.Count(s.Content, "\n") + 1
		if s.LineCount != expected {
			return &invariant.Violation{
				InvariantID: "B1",
				Message: fmt.Sprintf(
					"LineCount %d != strings.Count(Content,\"\\n\")+1 = %d",
					s.LineCount, expected,
				),
			}
		}
	}

	// S1: every Selected cell's BufOffset must lie inside some cursor selection range.
	if len(s.Cursors) > 0 {
		for lineIdx, line := range s.Cells {
			for colIdx, c := range line {
				if !c.Selected || c.BufOffset < 0 {
					continue
				}
				inRange := false
				for _, cur := range s.Cursors {
					lo, hi := cur.SelectionRange()
					if lo == hi {
						continue // no selection
					}
					if c.BufOffset >= lo && c.BufOffset < hi {
						inRange = true
						break
					}
				}
				if !inRange {
					return &invariant.Violation{
						InvariantID: "S1",
						Message: fmt.Sprintf(
							"line %d col %d: Selected cell BufOffset %d not in any cursor selection",
							lineIdx, colIdx, c.BufOffset,
						),
					}
				}
			}
		}
	}

	return nil
}

// CheckTransition runs textedit-domain L1 transition invariants.
// Returns all violations found.
func CheckTransition(prev snapshot.Snapshot, msg any, next snapshot.Snapshot) []invariant.Violation {
	var vs []invariant.Violation
	typeName := fmt.Sprintf("%T", msg)

	// B2: buffer version monotone non-decreasing; strictly increases when content changes.
	// Skip when either side is at the initial version (1).
	if next.BufferVersion > 1 && prev.BufferVersion > 1 {
		if next.BufferVersion < prev.BufferVersion {
			vs = append(vs, invariant.Violation{
				InvariantID: "B2",
				Message:     fmt.Sprintf("BufferVersion decreased: %d → %d", prev.BufferVersion, next.BufferVersion),
			})
		} else if next.Content != prev.Content && next.BufferVersion == prev.BufferVersion {
			vs = append(vs, invariant.Violation{
				InvariantID: "B2",
				Message:     fmt.Sprintf("Content changed but BufferVersion unchanged at %d", prev.BufferVersion),
			})
		}
	}

	// NL-SEL: a selection whose boundary lands on '\n' must not consume it.
	// If cursor[i]'s SelectionEnd() pointed to '\n' in prev, next.Content must
	// retain at least as many '\n' starting from SelectionStart() as prev had
	// starting from SelectionEnd(). Covers all selection-consuming commands
	// (delete-left/right, insert-char, newline, cut, paste) via the shared
	// selectionEndInclusive chokepoint.
	if next.Content != prev.Content {
		for i, c := range prev.Cursors {
			if !c.HasSelection() {
				continue
			}
			end := c.SelectionEnd()
			if end >= len(prev.Content) || prev.Content[end] != '\n' {
				continue
			}
			want := strings.Count(prev.Content[end:], "\n")
			start := c.SelectionStart()
			got := 0
			if start <= len(next.Content) {
				got = strings.Count(next.Content[start:], "\n")
			}
			if got < want {
				vs = append(vs, invariant.Violation{
					InvariantID: "NL-SEL",
					Message: fmt.Sprintf(
						"cursor[%d] selection [%d,%d) ended at '\\n': "+
							"'\\n' count from SelectionStart dropped (%d → %d); "+
							"prev=%q next=%q",
						i, start, end, want, got,
						invariant.Trunc(prev.Content, 60),
						invariant.Trunc(next.Content, 60),
					),
				})
			}
		}
	}

	_ = typeName // reserved for future textedit transition invariants
	return vs
}
