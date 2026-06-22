package invariant

import (
	"fmt"
	"strings"
	"unicode/utf8"

	"charm.land/lipgloss/v2"

	"rune/pkg/editor/cursor"
	"rune/pkg/editor/display"
	"rune/pkg/ui/components/footer"
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

	// Editor structural state (from FuzzCursors / fuzz accessors)
	Cursors       []cursor.Cursor
	BufferVersion uint64
	LineCount     int

	// Display pipeline snapshots (for D-family and WRAP-RT/SPAN-COVER/COORD-RT)
	Display display.DisplaySnapshot
	Wrap    display.WrapSnapshot
	Syntax  display.SyntaxSnapshot

	// Tab bar state
	Tabs         []TabInfo
	ActiveTabIdx int
	TabActive    []bool // Tab.Active flags (for TAB-SET)
	TabCount     int    // current number of open tabs
	TabLimit     int    // enforced hard cap (0 = uncapped)

	// SHADOW: set by driver (not by FuzzInspect); empty string = not yet checked
	MirrorContent string

	// CloseFileKeyPressed: set by driver (not by FuzzInspect) when the message was a CloseFile key press.
	CloseFileKeyPressed bool

	// File / persistence
	ActiveFilePath string // m.filePath; empty = Untitled/unsaved file
	EditorPath     string // same as filePath; named for clarity in invariants
	DocID          int64
	FlushGen       uint64
	SaveSnapshot   []byte // activeSave.SavedContent — content captured at save-start
	SaveInFlight   bool

	// Layout (for L1/L2/P1)
	Frame  string
	Width  int
	Height int

	// Guard / chord / focus
	HasDirtyFile    bool
	ActiveTabDirty  bool // true iff the currently active tab has unsaved changes
	GuardVisible    bool
	GuardKind       footer.GuardKind
	GuardOptionCount int
	ChordPending    bool
	FocusPane       int // 0=tree,1=tabs,2=center,3=title,4=chat
	AppQuitting     bool

	// Filetree (for FT-BOUNDS)
	FiletreeCursor int
	FiletreeLen    int
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

	// R8: no cell may carry a newline or carriage-return rune.
	for lineIdx, line := range s.Cells {
		for colIdx, c := range line {
			if c.Rune == '\n' || c.Rune == '\r' {
				return &Violation{
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
				return &Violation{
					InvariantID: "R9",
					Message: fmt.Sprintf(
						"line %d col %d: decorative cell (BufOffset==-1) is Selected=%v Cursor=%v",
						lineIdx, colIdx, c.Selected, c.Cursor,
					),
				}
			}
		}
	}

	// C1: cursors sorted by SelectionStart, non-overlapping.
	for i := 1; i < len(s.Cursors); i++ {
		prev := s.Cursors[i-1]
		cur := s.Cursors[i]
		if prev.SelectionEnd() > cur.SelectionStart() {
			return &Violation{
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
				return &Violation{
					InvariantID: "C2",
					Message:     fmt.Sprintf("cursor[%d] has non-positive ID %d", i, c.ID),
				}
			}
			if j, dup := seen[c.ID]; dup {
				return &Violation{
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
			return &Violation{
				InvariantID: "C3",
				Message: fmt.Sprintf(
					"cursor[%d] Position %d out of [0, %d]", i, c.Position, contentLen,
				),
			}
		}
		if c.Anchor < 0 || c.Anchor > contentLen {
			return &Violation{
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
			return &Violation{
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
					return &Violation{
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

	// L1: every frame line's display width ≤ terminal width (no overflow).
	// Uses lipgloss.Width which strips ANSI codes and computes terminal display width.
	if s.Width > 0 && s.Frame != "" {
		for i, line := range strings.Split(s.Frame, "\n") {
			w := lipgloss.Width(line)
			if w > s.Width {
				return &Violation{
					InvariantID: "L1",
					Message: fmt.Sprintf(
						"frame line %d display-width %d > terminal width %d", i, w, s.Width,
					),
				}
			}
		}
	}

	// L2: frame line count ≤ terminal height.
	if s.Height > 0 && s.Frame != "" {
		lines := strings.Count(s.Frame, "\n") + 1
		if lines > s.Height {
			return &Violation{
				InvariantID: "L2",
				Message: fmt.Sprintf(
					"frame has %d lines > terminal height %d", lines, s.Height,
				),
			}
		}
	}

	// TAB-SET: exactly one tab is Active when tabs non-empty; all paths unique.
	{
		activeCount := 0
		for _, a := range s.TabActive {
			if a {
				activeCount++
			}
		}
		if len(s.TabActive) > 0 && activeCount != 1 {
			return &Violation{
				InvariantID: "TAB-SET",
				Message:     fmt.Sprintf("expected exactly 1 active tab, got %d", activeCount),
			}
		}
	}

	// EDITOR-TAB-COH: editor path equals the active tab's path.
	if len(s.Tabs) > 0 && s.ActiveTabIdx >= 0 && s.ActiveTabIdx < len(s.Tabs) {
		activeTabPath := s.Tabs[s.ActiveTabIdx].Path
		if s.EditorPath != activeTabPath {
			return &Violation{
				InvariantID: "EDITOR-TAB-COH",
				Message: fmt.Sprintf(
					"EditorPath %q != Tabs[%d].Path %q",
					s.EditorPath, s.ActiveTabIdx, activeTabPath,
				),
			}
		}
	}

	// FT-BOUNDS: filetree cursor must be in [0, FiletreeLen) or 0 when empty.
	if s.FiletreeLen > 0 {
		if s.FiletreeCursor < 0 || s.FiletreeCursor >= s.FiletreeLen {
			return &Violation{
				InvariantID: "FT-BOUNDS",
				Message: fmt.Sprintf(
					"FiletreeCursor %d out of [0, %d)", s.FiletreeCursor, s.FiletreeLen,
				),
			}
		}
	}

	// TR-focus-valid: FocusPane must be one of the known pane enum values (0–4).
	const maxPane = 4
	if s.FocusPane < 0 || s.FocusPane > maxPane {
		return &Violation{
			InvariantID: "TR-focus-valid",
			Message:     fmt.Sprintf("FocusPane %d not in [0, %d]", s.FocusPane, maxPane),
		}
	}

	// GUARD-SYNC: GuardOptionCount > 0 ⟺ GuardVisible; both clear atomically.
	if s.GuardVisible != (s.GuardOptionCount > 0) {
		return &Violation{
			InvariantID: "GUARD-SYNC",
			Message: fmt.Sprintf(
				"GuardVisible=%v but GuardOptionCount=%d (must agree)",
				s.GuardVisible, s.GuardOptionCount,
			),
		}
	}

	// SAVE-SM: at most one in-flight save; FlushGen is checked via B2 (monotone) in CheckTransition.
	// (SaveInFlight is a bool so it's trivially ≤ 1 — flag if true while SaveSnapshot is nil.)
	if s.SaveInFlight && s.SaveSnapshot == nil {
		return &Violation{
			InvariantID: "SAVE-SM",
			Message:     "save InFlight but SavedContent is nil (missing save identity)",
		}
	}

	// ---- Display family (D1–D6, WRAP-RT, SPAN-COVER) ----

	// D1: for each Rendered span with non-nil CellMap: len(CellMap) == RuneCount(Text).
	for lineIdx, dline := range s.Display.Lines {
		for spanIdx, sp := range dline.Spans {
			if sp.State != display.Rendered || sp.CellMap == nil {
				continue
			}
			want := utf8.RuneCountInString(sp.Text)
			if len(sp.CellMap) != want {
				return &Violation{
					InvariantID: "D1",
					Message: fmt.Sprintf(
						"display line %d span %d: CellMap len %d != rune count %d (text %q)",
						lineIdx, spanIdx, len(sp.CellMap), want, trunc(sp.Text, 40),
					),
				}
			}
		}
	}

	// D2: a Rendered span with non-empty Text must not have a nil CellMap.
	for lineIdx, dline := range s.Display.Lines {
		for spanIdx, sp := range dline.Spans {
			if sp.State == display.Rendered && sp.Text != "" && sp.CellMap == nil {
				return &Violation{
					InvariantID: "D2",
					Message: fmt.Sprintf(
						"display line %d span %d: Rendered span %q has nil CellMap",
						lineIdx, spanIdx, trunc(sp.Text, 40),
					),
				}
			}
		}
	}

	// D3: every span must have BufferStart ≤ BufferEnd.
	for lineIdx, dline := range s.Display.Lines {
		for spanIdx, sp := range dline.Spans {
			if sp.BufferStart > sp.BufferEnd {
				return &Violation{
					InvariantID: "D3",
					Message: fmt.Sprintf(
						"display line %d span %d: BufferStart %d > BufferEnd %d",
						lineIdx, spanIdx, sp.BufferStart, sp.BufferEnd,
					),
				}
			}
		}
	}

	// D5: a Revealed span's Text must equal raw buffer bytes [BufferStart:BufferEnd].
	for lineIdx, dline := range s.Display.Lines {
		for spanIdx, sp := range dline.Spans {
			if sp.State != display.Revealed || sp.Text == "" {
				continue
			}
			if sp.BufferStart < 0 || sp.BufferEnd > len(s.Content) || sp.BufferStart > sp.BufferEnd {
				continue // bounds already caught by D3 / M2
			}
			want := s.Content[sp.BufferStart:sp.BufferEnd]
			if sp.Text != want {
				return &Violation{
					InvariantID: "D5",
					Message: fmt.Sprintf(
						"display line %d span %d: Revealed text %q != buffer[%d:%d] %q",
						lineIdx, spanIdx,
						trunc(sp.Text, 40), sp.BufferStart, sp.BufferEnd,
						trunc(want, 40),
					),
				}
			}
		}
	}

	// D6: len(Display.Lines) == Display.TotalRows (one DisplayLine per wrap row).
	if s.Display.TotalRows > 0 && len(s.Display.Lines) != s.Display.TotalRows {
		return &Violation{
			InvariantID: "D6",
			Message: fmt.Sprintf(
				"len(Display.Lines)=%d != Display.TotalRows=%d",
				len(s.Display.Lines), s.Display.TotalRows,
			),
		}
	}

	// WRAP-RT: per model line, concatenated WrapSegment span texts equal the syntax-line text.
	for lineIdx, sline := range s.Syntax.Lines {
		syntaxText := ""
		for _, sp := range sline.Spans {
			syntaxText += sp.Text
		}
		wrapText := ""
		for _, seg := range s.Wrap.Segments {
			if seg.ModelLine != lineIdx {
				continue
			}
			for _, sp := range seg.Spans {
				wrapText += sp.Text
			}
		}
		if wrapText != syntaxText {
			return &Violation{
				InvariantID: "WRAP-RT",
				Message: fmt.Sprintf(
					"model line %d: wrap segments text %q != syntax line text %q",
					lineIdx, trunc(wrapText, 60), trunc(syntaxText, 60),
				),
			}
		}
	}

	// SPAN-COVER: per syntax line, span [BufferStart,BufferEnd) tiles the line with no gap/overlap.
	for lineIdx, sline := range s.Syntax.Lines {
		if len(sline.Spans) == 0 {
			continue
		}
		// Compute expected line start from Content.
		lineStart := 0
		for i, ch := range s.Content {
			if lineIdx == 0 {
				break
			}
			if ch == '\n' {
				lineIdx--
				lineStart = i + 1
			}
		}
		// Find line end.
		lineEnd := strings.Index(s.Content[lineStart:], "\n")
		if lineEnd < 0 {
			lineEnd = len(s.Content)
		} else {
			lineEnd += lineStart
		}

		// Check coverage within [lineStart, lineEnd].
		pos := lineStart
		for spanIdx, sp := range sline.Spans {
			if sp.BufferStart < pos {
				return &Violation{
					InvariantID: "SPAN-COVER",
					Message: fmt.Sprintf(
						"syntax line span[%d]: BufferStart %d < expected %d (overlap)",
						spanIdx, sp.BufferStart, pos,
					),
				}
			}
			if sp.BufferStart > pos {
				return &Violation{
					InvariantID: "SPAN-COVER",
					Message: fmt.Sprintf(
						"syntax line span[%d]: gap at [%d, %d) in line [%d, %d)",
						spanIdx, pos, sp.BufferStart, lineStart, lineEnd,
					),
				}
			}
			pos = sp.BufferEnd
		}
	}

	return nil
}

// CheckDataLossInvariants checks DL1: VFS content must equal buffer content
// immediately after an autosave snapshot settles.
// vfsContent is the result of store.Content(snap.DocID), passed by the driver
// so this package remains docstate-free (N2).
// A missing (empty) VFS content when snap.DocID != 0 is a hard DL1 violation —
// the autosave must have committed something.
func CheckDataLossInvariants(s Snapshot, vfsContent string) *Violation {
	if s.DocID == 0 {
		return nil // no VFS doc yet — untitled without a scratch allocation
	}
	if vfsContent == "" {
		return &Violation{
			InvariantID: "DL1",
			Message: fmt.Sprintf(
				"autosave settled but VFS has no content for docID=%d (buffer[:%d]=%q)",
				s.DocID, min(len(s.Content), 40), trunc(s.Content, 40),
			),
		}
	}
	if vfsContent != s.Content {
		return &Violation{
			InvariantID: "DL1",
			Message: fmt.Sprintf(
				"VFS[:%d]=%q != buffer[:%d]=%q",
				min(len(vfsContent), 40), trunc(vfsContent, 40),
				min(len(s.Content), 40), trunc(s.Content, 40),
			),
		}
	}
	return nil
}

// CheckTransition runs L1 invariants that compare (prev, msg, next) triples.
// msg is passed as any so the invariant package does not need to import tea or workspace.
// Returns all violations found (first-wins is enforced by the driver).
func CheckTransition(prev Snapshot, msg any, next Snapshot) []Violation {
	var vs []Violation

	add := func(id, message string) {
		vs = append(vs, Violation{InvariantID: id, Message: message})
	}

	typeName := fmt.Sprintf("%T", msg)

	// B2: buffer version monotone non-decreasing; strictly increases when content changes.
	// Skip when either side is at the initial version (1) — buffer.New always starts at 1
	// so a file-load event sets prev→version1 and next→version1 with different content,
	// which is valid initialization, not a mutation bug.
	if next.BufferVersion > 1 && prev.BufferVersion > 1 {
		if next.BufferVersion < prev.BufferVersion {
			add("B2", fmt.Sprintf("BufferVersion decreased: %d → %d", prev.BufferVersion, next.BufferVersion))
		} else if next.Content != prev.Content && next.BufferVersion == prev.BufferVersion {
			add("B2", fmt.Sprintf("Content changed but BufferVersion unchanged at %d", prev.BufferVersion))
		}
	}

	// RESIZE-INV: a WindowSizeMsg must not mutate buffer content, cursor positions, or dirty state.
	if typeName == "tea.WindowSizeMsg" {
		if next.Content != prev.Content {
			add("RESIZE-INV", fmt.Sprintf(
				"Content changed on resize: %q → %q",
				trunc(prev.Content, 40), trunc(next.Content, 40),
			))
		}
		if prev.HasDirtyFile != next.HasDirtyFile {
			add("RESIZE-INV", fmt.Sprintf(
				"HasDirtyFile changed on resize: %v → %v", prev.HasDirtyFile, next.HasDirtyFile,
			))
		}
	}

	// SAVE-NOMUT: a save message must not mutate the buffer content.
	if typeName == "workspace.FileSavedMsg" {
		if next.Content != prev.Content {
			add("SAVE-NOMUT", fmt.Sprintf(
				"Content changed during save: %q → %q",
				trunc(prev.Content, 40), trunc(next.Content, 40),
			))
		}
		// TR-dirty-clear: after a save, active file must not be dirty.
		if next.HasDirtyFile {
			add("TR-dirty-clear", "active file still dirty after FileSavedMsg settled")
		}
	}

	// G2: DataLossGuardResponseMsg must clear the guard.
	if typeName == "footer.DataLossGuardResponseMsg" && next.GuardVisible {
		add("G2", "GuardVisible still true after DataLossGuardResponseMsg")
	}

	// G1: dirty file + ConfirmQuitMsg → guard must appear.
	if typeName == "footer.ConfirmQuitMsg" && prev.HasDirtyFile && !next.GuardVisible {
		add("G1", "dirty file + ConfirmQuitMsg did not raise guard")
	}

	// G3: dirty active tab + CloseFile key → guard must appear (unless guard already active).
	if next.CloseFileKeyPressed && prev.ActiveTabDirty && !prev.GuardVisible && !next.GuardVisible {
		add("G3", "dirty active tab + CloseFile key (^w) did not raise guard")
	}

	// TR-cursor-not-dirty: a key press that does not change buffer content must not
	// set the dirty flag. Cursor movement, selection extension, and similar
	// navigation-only operations must never mark the file dirty.
	if typeName == "tea.KeyPressMsg" && !prev.HasDirtyFile && next.HasDirtyFile && next.Content == prev.Content {
		add("TR-cursor-not-dirty", "key press set dirty flag without any content change")
	}

	return vs
}
