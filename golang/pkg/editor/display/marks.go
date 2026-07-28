package display

// InlineMarks is a set of composable inline text decorations (bold, italic,
// strikethrough) that apply ON TOP OF a span's content Kind. Keeping decorations
// out of Kind lets a single span carry both a content role and decorations — e.g.
// **[x](y)** is one span with Kind=TokenLink and MarkBold set. Zero value = none.
type InlineMarks uint8

const (
	MarkBold InlineMarks = 1 << iota
	MarkItalic
	MarkStrikethrough
)

// Has reports whether all bits in m are set.
func (s InlineMarks) Has(m InlineMarks) bool { return s&m == m }
