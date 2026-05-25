package display

import (
	"rune/pkg/editor/buffer"
	"rune/pkg/editor/coords"
	"rune/pkg/editor/cursor"
)

type TokenKind int

const (
	TokenText TokenKind = iota
	TokenHeading
	TokenBold
	TokenItalic
	TokenStrikethrough
	TokenInlineCode
	TokenLink
	TokenImage
	TokenBlockquote
	TokenTaskList
	TokenHorizontalRule
	TokenCodeFence
	TokenTable
	TokenMathBlock
	TokenInlineMath
	TokenFrontmatter
	TokenCallout
	TokenListMarker
	TokenTag
	TokenRawURL
	TokenHighlight
)

type RevealState int

const (
	Rendered RevealState = iota
	Revealed
)

type SyntaxSpan struct {
	Text        string
	Kind        TokenKind
	State       RevealState
	BufferStart int
	BufferEnd   int
	Language    string
	BlockID     int
	BlockStart  int
	BlockEnd    int
}

type SyntaxLine struct {
	Spans []SyntaxSpan
}

type OffsetDelta struct {
	BufferOffset int
	Delta        int
}

type SyntaxSnapshot struct {
	Lines  []SyntaxLine
	Deltas []OffsetDelta
}

func (s SyntaxSnapshot) BufferToSyntax(bp coords.BufferPoint) coords.SyntaxPoint {
	return coords.SyntaxPoint{Line: bp.Line, Col: bp.Col}
}

func (s SyntaxSnapshot) SyntaxToBuffer(sp coords.SyntaxPoint) coords.BufferPoint {
	return coords.BufferPoint{Line: sp.Line, Col: sp.Col}
}

func (s SyntaxSnapshot) SyntaxColWidth(line int) int {
	if line < 0 || line >= len(s.Lines) {
		return 0
	}
	width := 0
	for _, span := range s.Lines[line].Spans {
		width += len(span.Text)
	}
	return width
}

type SyntaxMap struct {
	lastBufVer    uint64
	lastCursorPos coords.BufferPoint
}

func NewSyntaxMap() SyntaxMap {
	return SyntaxMap{}
}

func (m SyntaxMap) Sync(buf buffer.Buffer, cursors cursor.CursorSet) (SyntaxMap, SyntaxSnapshot) {
	lines := make([]SyntaxLine, buf.LineCount())
	for i := 0; i < buf.LineCount(); i++ {
		text := buf.Line(i)
		start := buf.LineStart(i)
		end := start + len(text)
		lines[i] = SyntaxLine{
			Spans: []SyntaxSpan{
				{
					Text:        text,
					Kind:        TokenText,
					State:       Revealed,
					BufferStart: start,
					BufferEnd:   end,
				},
			},
		}
	}

	m.lastBufVer = buf.Version()
	if !buf.Empty() && cursors.Len() > 0 {
		m.lastCursorPos = buf.OffsetToLineCol(cursors.Primary().Position)
	}

	return m, SyntaxSnapshot{
		Lines:  lines,
		Deltas: nil,
	}
}
