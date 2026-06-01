package display

import (
	"sort"
	"strings"

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
	TokenWikiLink
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

// TableRoleKind distinguishes header, separator, and body lines in a table block.
type TableRoleKind int

const (
	TableRoleBody TableRoleKind = iota
	TableRoleHeader
	TableRoleSeparator
)

type SyntaxSpan struct {
	Text         string
	Kind         TokenKind
	State        RevealState
	BufferStart  int
	BufferEnd    int
	CellMap      []CellMapping // per-visual-byte buffer offsets for Rendered spans; nil for Revealed
	Language     string
	BlockID      int
	BlockStart   int
	BlockEnd     int
	AltText      string // image alt text
	ImagePath    string // image path/URL
	EmbedRef     string // embed reference (e.g. [[filename]])
	CalloutKind  string // callout type (e.g. "note", "warning")
	HeadingLevel int    // heading level (1-6), 0 for non-headings
	TableRole    TableRoleKind
	// Wiki link metadata (set for TokenWikiLink spans)
	WikiLinkTarget  string // resolved file path for wiki links
	WikiLinkIsImage bool   // true for embedded images ![[image.png]]
	LinkURL         string // link destination for TokenLink spans ([text](url))
}

// FrontmatterMode controls how frontmatter is displayed in rendered mode.
type FrontmatterMode int

const (
	FrontmatterCollapsed FrontmatterMode = iota
	FrontmatterSource
	FrontmatterHidden
)

type SyntaxLine struct {
	Spans []SyntaxSpan
}

type OffsetDelta struct {
	BufferOffset int
	Delta        int
}

// hiddenRange represents a non-cursor-legal buffer column range.
type hiddenRange struct {
	start   int // first non-legal col (inclusive)
	end     int // first legal col after (exclusive)
	clampTo int // buffer col to clamp to
}

// lineConversion holds per-line data for coordinate conversion.
type lineConversion struct {
	deltas []OffsetDelta // sorted by BufferOffset, monotonically increasing Delta
	hidden []hiddenRange // sorted by start
}

type SyntaxSnapshot struct {
	Lines     []SyntaxLine
	Deltas    []OffsetDelta // global monotonic (for external consumers)
	lineConvs []lineConversion
}

// BufferToSyntax converts a buffer-space point to syntax-space.
// Positions inside hidden delimiters are clamped to cursor-legal positions.
func (s SyntaxSnapshot) BufferToSyntax(bp coords.BufferPoint) coords.SyntaxPoint {
	if bp.Line < 0 || bp.Line >= len(s.lineConvs) {
		return coords.SyntaxPoint{Line: bp.Line, Col: bp.Col}
	}

	lc := s.lineConvs[bp.Line]
	if len(lc.deltas) == 0 {
		return coords.SyntaxPoint{Line: bp.Line, Col: bp.Col}
	}

	// Clamp position out of hidden ranges
	col := clampCol(bp.Col, lc.hidden)

	// Binary search for the last delta entry with BufferOffset <= col
	idx := sort.Search(len(lc.deltas), func(i int) bool {
		return lc.deltas[i].BufferOffset > col
	}) - 1

	delta := 0
	if idx >= 0 {
		delta = lc.deltas[idx].Delta
	}

	syntaxCol := col - delta
	if syntaxCol < 0 {
		syntaxCol = 0
	}

	return coords.SyntaxPoint{Line: bp.Line, Col: syntaxCol}
}

// SyntaxToBuffer converts a syntax-space point back to buffer-space.
func (s SyntaxSnapshot) SyntaxToBuffer(sp coords.SyntaxPoint) coords.BufferPoint {
	if sp.Line < 0 || sp.Line >= len(s.lineConvs) {
		return coords.BufferPoint{Line: sp.Line, Col: sp.Col}
	}

	lc := s.lineConvs[sp.Line]
	if len(lc.deltas) == 0 {
		return coords.BufferPoint{Line: sp.Line, Col: sp.Col}
	}

	// Reverse mapping: find the delta region where sp.Col falls.
	// For each delta entry at BufferOffset B with Delta D:
	//   syntax col at B = B - D
	// We need the last entry i where (B_i - D_i) <= sp.Col
	idx := -1
	for i, d := range lc.deltas {
		syntaxAtEntry := d.BufferOffset - d.Delta
		if syntaxAtEntry <= sp.Col {
			idx = i
		} else {
			break
		}
	}

	delta := 0
	if idx >= 0 {
		delta = lc.deltas[idx].Delta
	}

	return coords.BufferPoint{Line: sp.Line, Col: sp.Col + delta}
}

// clampCol adjusts a buffer column that falls inside a hidden range.
func clampCol(col int, hidden []hiddenRange) int {
	for _, h := range hidden {
		if col >= h.start && col < h.end {
			return h.clampTo
		}
		if h.start > col {
			break // sorted, no more matches
		}
	}
	return col
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

// NewSyntaxSnapshotFromLines creates a SyntaxSnapshot with identity coordinate
// mapping (no hidden ranges, no deltas). Used for testing wrap behavior in
// isolation without markdown token folding.
func NewSyntaxSnapshotFromLines(lines []SyntaxLine) SyntaxSnapshot {
	convs := make([]lineConversion, len(lines))
	// All lineConversion entries default to empty deltas/hidden = identity mapping
	return SyntaxSnapshot{
		Lines:     lines,
		lineConvs: convs,
	}
}

type SyntaxMap struct {
	lastBufVer      uint64
	lastCursorPos   coords.BufferPoint
	FrontmatterMode FrontmatterMode
}

func NewSyntaxMap() SyntaxMap {
	return SyntaxMap{FrontmatterMode: FrontmatterCollapsed}
}

func (m SyntaxMap) Sync(buf buffer.Buffer, cursors cursor.CursorSet) (SyntaxMap, SyntaxSnapshot) {
	m.lastBufVer = buf.Version()
	cursorLine := -1
	if !buf.Empty() && cursors.Len() > 0 {
		m.lastCursorPos = buf.OffsetToLineCol(cursors.Primary().Position)
		cursorLine = m.lastCursorPos.Line
	}

	content := buf.Content()
	parsed, blocks := parseMarkdownAdvanced(content)
	advBlocks := parseAdvancedBlocks(content, m.FrontmatterMode)
	blocks = append(blocks, advBlocks...)

	// Parse YAML frontmatter to detect malformed content for error display.
	var fmError string
	if m.FrontmatterMode != FrontmatterHidden {
		docLines := strings.Split(content, "\n")
		if fmEnd := detectFrontmatter(docLines); fmEnd >= 0 {
			if _, err := parseFrontmatterYAML(docLines, fmEnd); err != nil {
				fmError = err.Error()
			}
		}
	}

	// Build a line→block index for fast lookup
	lineToBlock := buildLineToBlockIndex(blocks, buf.LineCount())

	lines := make([]SyntaxLine, buf.LineCount())
	lineConvs := make([]lineConversion, buf.LineCount())
	var allDeltas []OffsetDelta

	for i := 0; i < buf.LineCount(); i++ {
		lineText := buf.Line(i)
		lineStart := buf.LineStart(i)

		// Check if this line belongs to a block
		if blkIdx := lineToBlock[i]; blkIdx >= 0 {
			block := blocks[blkIdx]
			revealed := shouldRevealBlock(block, cursorLine)

			var mdSpans []mdSpan
			if i < len(parsed) {
				mdSpans = parsed[i].spans
			}

			spans := blockSpansForLine(block, i, lineText, lineStart, revealed, m.FrontmatterMode, fmError, mdSpans)
			lines[i] = SyntaxLine{Spans: spans}
			// Block lines in rendered mode with hidden delimiters need deltas
			needsHiddenLineDelta := false
			if !revealed {
				switch block.kind {
				case TokenCodeFence:
					needsHiddenLineDelta = i == block.startLine || i == block.endLine
				case TokenMathBlock:
					needsHiddenLineDelta = i == block.startLine || i == block.endLine
				case TokenFrontmatter:
					if m.FrontmatterMode == FrontmatterCollapsed {
						needsHiddenLineDelta = i > block.startLine
					} else if m.FrontmatterMode == FrontmatterHidden {
						needsHiddenLineDelta = true
					}
				}
			}
			if needsHiddenLineDelta && len(lineText) > 0 {
				lc := lineConversion{
					deltas: []OffsetDelta{{
						BufferOffset: len(lineText),
						Delta:        len(lineText),
					}},
					hidden: []hiddenRange{{
						start:   0,
						end:     len(lineText),
						clampTo: len(lineText),
					}},
				}
				lineConvs[i] = lc
				allDeltas = append(allDeltas, OffsetDelta{
					BufferOffset: lineStart + len(lineText),
					Delta:        len(lineText),
				})
			}
			continue
		}

		if i < len(parsed) && len(parsed[i].spans) > 0 {
			sl, lc := buildSyntaxLine(
				lineText, lineStart, i, cursorLine, m.lastCursorPos.Col, parsed[i].spans,
			)
			lines[i] = sl
			lineConvs[i] = lc
			// Add to global deltas with absolute offsets
			for _, d := range lc.deltas {
				allDeltas = append(allDeltas, OffsetDelta{
					BufferOffset: lineStart + d.BufferOffset,
					Delta:        d.Delta,
				})
			}
		} else {
			lines[i] = SyntaxLine{
				Spans: []SyntaxSpan{{
					Text:        lineText,
					Kind:        TokenText,
					State:       Revealed,
					BufferStart: lineStart,
					BufferEnd:   lineStart + len(lineText),
				}},
			}
		}
	}

	return m, SyntaxSnapshot{
		Lines:     lines,
		Deltas:    allDeltas,
		lineConvs: lineConvs,
	}
}

// buildLineToBlockIndex creates a mapping from line index to block index.
// Returns a slice where lineToBlock[i] = index into blocks, or -1 if no block.
func buildLineToBlockIndex(blocks []mdBlock, lineCount int) []int {
	index := make([]int, lineCount)
	for i := range index {
		index[i] = -1
	}
	for bi, block := range blocks {
		for line := block.startLine; line <= block.endLine && line < lineCount; line++ {
			index[line] = bi
		}
	}
	return index
}

// buildSyntaxLine produces spans and conversion data for a single line.
func buildSyntaxLine(
	lineText string, lineStart, lineIdx, cursorLine, cursorCol int, mdSpans []mdSpan,
) (SyntaxLine, lineConversion) {
	// Sort spans by start position
	sorted := make([]mdSpan, len(mdSpans))
	copy(sorted, mdSpans)
	sort.Slice(sorted, func(i, j int) bool {
		return sorted[i].start < sorted[j].start
	})

	var spans []SyntaxSpan
	var deltas []OffsetDelta
	var hidden []hiddenRange
	pos := 0        // current buffer col in line
	accumDelta := 0 // accumulated hidden bytes

	for _, ms := range sorted {
		if ms.start < pos {
			continue // overlapping
		}

		reveal := shouldReveal(ms, lineIdx, cursorLine, cursorCol)

		// Emit text before this span
		if ms.start > pos {
			spans = append(spans, SyntaxSpan{
				Text:        lineText[pos:ms.start],
				Kind:        TokenText,
				State:       Revealed,
				BufferStart: lineStart + pos,
				BufferEnd:   lineStart + ms.start,
			})
		}

		if reveal {
			// Show raw text including delimiters
			raw := lineText[ms.start:ms.end]
			spans = append(spans, SyntaxSpan{
				Text:            raw,
				Kind:            ms.kind,
				State:           Revealed,
				BufferStart:     lineStart + ms.start,
				BufferEnd:       lineStart + ms.end,
				AltText:         spanAltText(ms),
				ImagePath:       spanImagePath(ms),
				EmbedRef:        spanEmbedRef(ms),
				CalloutKind:     spanCalloutKind(ms),
				HeadingLevel:    ms.level,
				WikiLinkTarget:  spanWikiLinkTarget(ms),
				WikiLinkIsImage: spanWikiLinkIsImage(ms),
				LinkURL:         spanLinkURL(ms),
			})
		} else {
			// Hide delimiters
			hiddenLeft := ms.delimLeft
			hiddenRight := ms.delimRight

			// Left hidden range: [ms.start, ms.start+hiddenLeft)
			if hiddenLeft > 0 {
				hidden = append(hidden, hiddenRange{
					start:   ms.start,
					end:     ms.start + hiddenLeft,
					clampTo: ms.start + hiddenLeft,
				})
				accumDelta += hiddenLeft
				deltas = append(deltas, OffsetDelta{
					BufferOffset: ms.start + hiddenLeft,
					Delta:        accumDelta,
				})
			}

			// Right hidden range: positions at/within the closing delimiter
			// clamp forward to after the delimiter
			if hiddenRight > 0 {
				rightStart := ms.end - hiddenRight
				hidden = append(hidden, hiddenRange{
					start:   rightStart,
					end:     ms.end,
					clampTo: ms.end,
				})
				accumDelta += hiddenRight
				deltas = append(deltas, OffsetDelta{
					BufferOffset: ms.end,
					Delta:        accumDelta,
				})
			}

			// Emit visible text with per-byte source mapping
			cm := buildInlineCellMap(lineStart+ms.start+hiddenLeft, len(ms.text))
			spans = append(spans, SyntaxSpan{
				Text:            ms.text,
				Kind:            ms.kind,
				State:           Rendered,
				BufferStart:     lineStart + ms.start,
				BufferEnd:       lineStart + ms.end,
				CellMap:         cm,
				AltText:         spanAltText(ms),
				ImagePath:       spanImagePath(ms),
				EmbedRef:        spanEmbedRef(ms),
				CalloutKind:     spanCalloutKind(ms),
				HeadingLevel:    ms.level,
				WikiLinkTarget:  spanWikiLinkTarget(ms),
				WikiLinkIsImage: spanWikiLinkIsImage(ms),
				LinkURL:         spanLinkURL(ms),
			})
		}

		pos = ms.end
	}

	// Emit remaining text
	if pos < len(lineText) {
		spans = append(spans, SyntaxSpan{
			Text:        lineText[pos:],
			Kind:        TokenText,
			State:       Revealed,
			BufferStart: lineStart + pos,
			BufferEnd:   lineStart + len(lineText),
		})
	}

	if len(spans) == 0 {
		spans = []SyntaxSpan{{
			Text:        lineText,
			Kind:        TokenText,
			State:       Revealed,
			BufferStart: lineStart,
			BufferEnd:   lineStart + len(lineText),
		}}
	}

	return SyntaxLine{Spans: spans}, lineConversion{deltas: deltas, hidden: hidden}
}

// shouldReveal determines if raw markdown should be shown based on cursor.
func shouldReveal(ms mdSpan, lineIdx, cursorLine, cursorCol int) bool {
	switch ms.kind {
	case TokenHeading, TokenBlockquote, TokenHorizontalRule, TokenTaskList, TokenCallout:
		// Line-level reveal
		return lineIdx == cursorLine
	case TokenInlineMath, TokenHighlight, TokenImage:
		// Per-token reveal: cursor must be within the span
		if lineIdx != cursorLine {
			return false
		}
		return cursorCol >= ms.start && cursorCol < ms.end
	default:
		if lineIdx != cursorLine {
			return false
		}
		return cursorCol >= ms.start && cursorCol < ms.end
	}
}

// spanAltText extracts alt text metadata for image spans.
func spanAltText(ms mdSpan) string {
	if ms.kind == TokenImage {
		return ms.text
	}
	if ms.kind == TokenWikiLink && ms.wikiLinkIsImage {
		return ms.wikiLinkLabel
	}
	return ""
}

// spanImagePath extracts image path metadata.
func spanImagePath(ms mdSpan) string {
	if ms.kind == TokenImage {
		return ms.linkURL
	}
	if ms.kind == TokenWikiLink && ms.wikiLinkIsImage {
		return ms.wikiLinkTarget
	}
	return ""
}

// spanEmbedRef extracts embed reference metadata.
func spanEmbedRef(ms mdSpan) string {
	if ms.kind == TokenImage && ms.delimLeft == 3 {
		return ms.linkURL
	}
	if ms.kind == TokenWikiLink && ms.wikiLinkIsImage {
		return ms.wikiLinkTarget
	}
	return ""
}

// spanCalloutKind extracts callout kind metadata.
func spanCalloutKind(ms mdSpan) string {
	if ms.kind == TokenCallout {
		return ms.linkURL
	}
	return ""
}

// spanWikiLinkTarget extracts wiki link target metadata.
func spanWikiLinkTarget(ms mdSpan) string {
	if ms.kind == TokenWikiLink {
		return ms.wikiLinkTarget
	}
	return ""
}

// spanWikiLinkIsImage extracts wiki link image flag.
func spanWikiLinkIsImage(ms mdSpan) bool {
	if ms.kind == TokenWikiLink {
		return ms.wikiLinkIsImage
	}
	return false
}

// spanLinkURL extracts link URL metadata for TokenLink spans.
func spanLinkURL(ms mdSpan) string {
	if ms.kind == TokenLink {
		return ms.linkURL
	}
	return ""
}
