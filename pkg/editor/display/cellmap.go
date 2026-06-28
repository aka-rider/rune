package display

import "unicode/utf8"

// CellMapping maps a single visual cell in a rendered span back to its source buffer offset.
// BufOffset is -1 for decorative/padding cells that have no buffer correspondence.
type CellMapping struct {
	BufOffset int
}

// buildInlineCellMap creates a CellMap for inline rendered spans where visible
// text maps 1:1 to buffer bytes starting at contentStart.
// For inline code `hello`, bold **hello`, etc., the visible text bytes correspond
// directly to buffer bytes after the left delimiter.
// CellMap is built per-visual-cell (per-rune), not per-byte.
func buildInlineCellMap(contentStart int, text []byte) []CellMapping {
	cm := make([]CellMapping, 0, utf8.RuneCountInString(string(text)))
	for i := 0; i < len(text); {
		cm = append(cm, CellMapping{BufOffset: contentStart + i})
		_, size := utf8.DecodeRuneInString(string(text[i:]))
		if size == 0 {
			size = 1
		}
		i += size
	}
	return cm
}
