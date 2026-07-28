package coords

// Buffer Space — raw byte positions in the UTF-8 document
type BufferOffset int
type BufferPoint struct {
	Line int // 0-indexed model line number
	Col  int // byte offset from start of that line
}

// Syntax Space — positions after markdown tokens are folded/expanded
type SyntaxOffset int
type SyntaxPoint struct {
	Line int // same model line as buffer (1:1 line mapping)
	Col  int // column in syntax space
}

// Wrap Space — positions after soft-wrap breaks are inserted. Frozen before
// table/image row expansion runs; NOT the same space as Display (below).
type WrapRow int
type WrapPoint struct {
	Row int // row in wrap-space (0-indexed from doc top), pre table/image expansion
	Col int // column within that wrapped segment
}

// Display Space — final terminal grid after table/image row expansion AND
// viewport slicing. DisplaySnapshot.TotalRows can exceed WrapSnapshot.TotalRows
// once expansion runs; converting a Display row to a Wrap row is NOT direct
// arithmetic — use DisplaySnapshot.RowToWrapRow.
type DisplayRow int
type DisplayPoint struct {
	Row int // row relative to viewport top, in POST-expansion display-space
	Col int // column (includes tab expansion)
}
