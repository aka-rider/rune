//go:build fuzzing

package textedit

import (
	"unicode/utf8"

	"rune/internal/fuzz/snapshot"
	"rune/pkg/editor/cursor"
)

// isEOLByte reports whether content[off] is a '\r' — the one byte position
// the renderer never gives a cursor cell. The structural truth (mirrors
// renderCellsForLines' EOL-cursor synthesis): only '\n' terminates a line
// (Buffer.Line splits on '\n' alone; '\r' is content), every line's last
// span ends AT its '\n' offset, and a cursor there always gets the synthetic
// EOL cell — including the '\n' of a CRLF pair (goal-column moves and End
// legitimately park the caret between '\r' and '\n'). A cursor on ANY '\r'
// — paired or lone (Backspace can strip the '\n' of a pair) — never has a
// cell, since no cell carries '\r' (R8). An earlier version also exempted
// the '\n' half of CRLF; FuzzHumanSession crasher 9e80ade998147f2b disproved
// that empirically (line-swap undo left an interior CRLF, Down's goal-column
// clamp landed the caret on its '\n', and the renderer — correctly — drew
// the EOL cell the exemption said couldn't exist).
func isEOLByte(content string, off int) bool {
	if off < 0 || off >= len(content) {
		return false
	}
	return content[off] == '\r'
}

// selectionEndRuneInclusive mirrors pkg/ui/components/textedit/commands_nav.go's
// selectionEndInclusive exactly, reimplemented against a content string (the
// checker has no buffer.Buffer): for a REVERSED cursor (Position < Anchor —
// selecting backward), the raw SelectionEnd (== Anchor, already passed in as
// hi) advances by one rune UNLESS that rune is '\n', so the anchor's own
// character is included in the rendered selection. Cursor.Reversed() is
// Position < Anchor, matching cur.SelectionRange()'s own lo/hi convention.
func selectionEndRuneInclusive(cur cursor.Cursor, content string, hi int) int {
	if !cur.Reversed() || hi >= len(content) {
		return hi
	}
	r, size := utf8.DecodeRuneInString(content[hi:])
	if r == '\n' {
		return hi
	}
	if size == 0 {
		return hi + 1
	}
	return hi + size
}

// eolExemptCursorCount returns len(s.CursorOffsets) minus however many of
// those offsets sit on an excluded \r/\n byte (isEOLByte) — R1's expected
// cursor-cell count once EOL-positioned cursors are accounted for.
func eolExemptCursorCount(s snapshot.Snapshot) int {
	n := 0
	for off := range s.CursorOffsets {
		if !isEOLByte(s.Content, off) {
			n++
		}
	}
	return n
}
