package invariant

import (
	"fmt"
	"os"

	"rune/pkg/ui/components/textedit"
)

// TabInfo represents a single tab's identity for invariant checking.
type TabInfo struct {
	Path string
	Name string
}

// Snapshot is a flat read-only value capturing workspace state after each
// settled message. FuzzInspect() on workspace.Model produces this.
type Snapshot struct {
	// Editor state
	Content       string
	Cells         [][]textedit.Cell // from renderCells() — same cells View() renders
	CursorOffsets map[int]bool      // active cursor byte offsets
	Focused       bool              // whether the editor pane has focus

	// Tab bar state
	Tabs         []TabInfo
	ActiveTabIdx int

	// SHADOW: set by driver (not by FuzzInspect); empty string = not yet checked
	MirrorContent string

	// Phase 2: DATA-LOSS support
	ActiveFilePath string // m.filePath; empty = Untitled/unsaved file
}

// Violation records a failed invariant check.
type Violation struct {
	InvariantID string
	Message     string
}

// trunc truncates s to at most n bytes, appending "…" if cut.
func trunc(s string, n int) string {
	if len(s) <= n {
		return s
	}
	return s[:n] + "…"
}

// CheckInvariants runs all Phase-0 and Phase-1 invariants against s.
// Returns the first violation found, or nil if all pass.
func CheckInvariants(s Snapshot) *Violation {
	// R1: cursor-cell count == active cursor count (only when editor is focused)
	if s.Focused && s.CursorOffsets != nil {
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
			return &Violation{
				InvariantID: "R1",
				Message: fmt.Sprintf(
					"cursor-cell count %d != active cursor count %d",
					cursorCellCount, activeCursorCount,
				),
			}
		}
	}

	// R2: every offset in CursorOffsets must appear as Cursor=true in at least one cell.
	// Cursor cells are only rendered when the editor is focused (textedit.go:870).
	if s.Focused && s.CursorOffsets != nil {
		// Build set of offsets that have a matching Cursor=true cell.
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
				return &Violation{
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
				return &Violation{
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
				return &Violation{
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
	contentLen := len(s.Content)
	for lineIdx, line := range s.Cells {
		for colIdx, c := range line {
			if c.BufOffset < -1 || c.BufOffset > contentLen {
				return &Violation{
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
	// Cursor cells are only rendered when the editor is focused (textedit.go:870).
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
				return &Violation{
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
	// Cursor cells are only rendered when the editor is focused (textedit.go:870).
	if s.Focused && s.CursorOffsets != nil {
		for lineIdx, line := range s.Cells {
			for colIdx, c := range line {
				if c.Cursor && c.BufOffset >= 0 && !s.CursorOffsets[c.BufOffset] {
					return &Violation{
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

	// SHADOW: buffer content must match the independently-maintained mirror
	if s.MirrorContent != "" && s.Content != s.MirrorContent {
		return &Violation{
			InvariantID: "SHADOW",
			Message: fmt.Sprintf("buffer %q != mirror %q",
				trunc(s.Content, 80), trunc(s.MirrorContent, 80)),
		}
	}

	// T1: no duplicate tab paths (empty paths are unsaved files — allowed to repeat).
	{
		seen := make(map[string]int, len(s.Tabs)) // path -> first index
		for i, tab := range s.Tabs {
			if tab.Path == "" {
				continue
			}
			if j, dup := seen[tab.Path]; dup {
				return &Violation{
					InvariantID: "T1",
					Message: fmt.Sprintf(
						"duplicate tab path %q at indices %d and %d",
						trunc(tab.Path, 120), j, i,
					),
				}
			}
			seen[tab.Path] = i
		}
	}

	// T2: active tab index in range.
	{
		n := len(s.Tabs)
		if n > 0 {
			if s.ActiveTabIdx < 0 || s.ActiveTabIdx >= n {
				return &Violation{
					InvariantID: "T2",
					Message: fmt.Sprintf(
						"ActiveTabIdx %d out of range [0, %d]",
						s.ActiveTabIdx, n-1,
					),
				}
			}
		} else {
			if s.ActiveTabIdx != 0 && s.ActiveTabIdx != -1 {
				return &Violation{
					InvariantID: "T2",
					Message: fmt.Sprintf(
						"ActiveTabIdx %d with empty tab list (want 0 or -1)",
						s.ActiveTabIdx,
					),
				}
			}
		}
	}

	// M1: when focused and cells are non-empty, at least one cursor offset must exist.
	if s.Focused && len(s.Cells) > 0 && len(s.CursorOffsets) < 1 {
		return &Violation{
			InvariantID: "M1",
			Message:     "editor focused with non-empty cells but CursorOffsets is empty",
		}
	}

	// M2: all offsets in CursorOffsets must be in [0, len(Content)] (EOF position valid).
	for off := range s.CursorOffsets {
		if off < 0 || off > contentLen {
			return &Violation{
				InvariantID: "M2",
				Message: fmt.Sprintf(
					"CursorOffset %d out of range [0, %d]",
					off, contentLen,
				),
			}
		}
	}

	return nil
}

// min returns the smaller of a and b.
func min(a, b int) int {
	if a < b {
		return a
	}
	return b
}

// CheckDataLossInvariants checks DATA-LOSS invariants that require disk access.
// Call only after the drain has settled following a save event.
func CheckDataLossInvariants(s Snapshot) *Violation {
	if s.ActiveFilePath == "" {
		return nil
	}
	diskData, err := os.ReadFile(s.ActiveFilePath)
	if err != nil {
		return nil // file not written yet; skip
	}
	if string(diskData) != s.Content {
		return &Violation{
			InvariantID: "DL1",
			Message: fmt.Sprintf(
				"file-on-disk[:%d]=%q != buffer[:%d]=%q",
				min(len(diskData), 40), trunc(string(diskData), 40),
				min(len(s.Content), 40), trunc(s.Content, 40),
			),
		}
	}
	return nil
}
