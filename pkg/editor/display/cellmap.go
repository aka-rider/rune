package display

// CellMapping maps a single visual cell in a rendered span back to its source buffer offset.
// BufOffset is -1 for decorative/padding cells that have no buffer correspondence.
type CellMapping struct {
	BufOffset int
}

// buildInlineCellMap creates a CellMap for inline rendered spans where visible
// text maps 1:1 to buffer bytes starting at contentStart.
// For inline code `hello`, bold **hello**, etc., the visible text bytes correspond
// directly to buffer bytes after the left delimiter.
func buildInlineCellMap(contentStart, textLen int) []CellMapping {
	cm := make([]CellMapping, textLen)
	for i := range cm {
		cm[i] = CellMapping{BufOffset: contentStart + i}
	}
	return cm
}
