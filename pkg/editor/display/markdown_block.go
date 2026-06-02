package display

import (
	"strings"

	"github.com/yuin/goldmark/ast"
	east "github.com/yuin/goldmark/extension/ast"
)

// mdBlock represents a multi-line markdown block element (code fence, table).
type mdBlock struct {
	kind      TokenKind
	id        int    // stable sequential ID
	startLine int    // first line of block (inclusive)
	endLine   int    // last line of block (inclusive)
	startOff  int    // byte offset of block start in document
	endOff    int    // byte offset of block end in document
	language  string // for code fences

	// Table metadata (only set for TokenTable blocks)
	colWidths  []int // max visual width per column
	alignments []int // 0=left, 1=center, 2=right (per goldmark)
	sepLine    int   // line index of separator row (-1 if none)
	headerEnd  int   // last line index of the header (startLine typically)
}

// walkFencedCodeBlock extracts block info for a fenced code block.
func walkFencedCodeBlock(
	node *ast.FencedCodeBlock,
	src []byte,
	lines []string,
	lineOffsets []int,
	blockID *int,
	blocks *[]mdBlock,
) {
	// FencedCodeBlock.Lines() gives content lines (between fences).
	// We need to find the opening and closing fence lines.
	if node.Lines().Len() == 0 && len(lines) == 0 {
		return
	}

	// Determine opening fence line.
	// The node doesn't directly expose the fence markers.
	// We find the opening fence by looking at the line BEFORE the first content line,
	// or by using the node's parent/sibling context.
	var firstContentStart int
	if node.Lines().Len() > 0 {
		firstContentStart = node.Lines().At(0).Start
	} else {
		// Empty code block - find by scanning from previous sibling
		firstContentStart = findEmptyFenceContentStart(node, src, lines, lineOffsets)
		if firstContentStart < 0 {
			return
		}
	}

	// The opening fence is the line just before the first content line.
	// For empty fences, the opening fence line is right before the closing.
	openLineIdx := findLineForOffset(lineOffsets, firstContentStart)
	if openLineIdx <= 0 {
		// If content starts at or near offset 0, opening fence is line 0
		// Check if line 0 looks like a fence
		if len(lines) > 0 && isFenceLine(lines[0]) {
			openLineIdx = 0
		} else {
			return
		}
	} else {
		openLineIdx-- // the line before content is the opening fence
	}

	// Verify the opening line is actually a fence
	if openLineIdx < 0 || openLineIdx >= len(lines) {
		return
	}
	if !isFenceLine(lines[openLineIdx]) {
		// Try the content start line itself for empty blocks
		openLineIdx = findLineForOffset(lineOffsets, firstContentStart)
		if openLineIdx < 0 || openLineIdx >= len(lines) || !isFenceLine(lines[openLineIdx]) {
			return
		}
	}

	// Determine the last content line
	var lastContentEnd int
	if node.Lines().Len() > 0 {
		lastSeg := node.Lines().At(node.Lines().Len() - 1)
		lastContentEnd = lastSeg.Stop
	} else {
		lastContentEnd = lineOffsets[openLineIdx] + len(lines[openLineIdx]) + 1
	}

	lastContentLineIdx := findLineForOffset(lineOffsets, lastContentEnd-1)
	if node.Lines().Len() == 0 {
		lastContentLineIdx = openLineIdx
	}

	// Closing fence is the line after the last content line
	closeLineIdx := lastContentLineIdx + 1
	if node.Lines().Len() > 0 {
		closeLineIdx = lastContentLineIdx + 1
	}
	if closeLineIdx >= len(lines) {
		closeLineIdx = len(lines) - 1
	}
	// Verify closing fence
	if closeLineIdx >= 0 && closeLineIdx < len(lines) && !isFenceLine(lines[closeLineIdx]) {
		// Unclosed fence - closeLineIdx is last content line
		closeLineIdx = lastContentLineIdx
	}

	// Extract language from opening fence
	lang := extractFenceLanguage(lines[openLineIdx])

	startOff := lineOffsets[openLineIdx]
	endOff := lineOffsets[closeLineIdx] + len(lines[closeLineIdx])

	*blockID++
	*blocks = append(*blocks, mdBlock{
		kind:      TokenCodeFence,
		id:        *blockID,
		startLine: openLineIdx,
		endLine:   closeLineIdx,
		startOff:  startOff,
		endOff:    endOff,
		language:  lang,
	})
}

// walkTable extracts block info for a table.
func walkTable(
	node *east.Table,
	src []byte,
	lines []string,
	lineOffsets []int,
	blockID *int,
	blocks *[]mdBlock,
) {
	// Find the line range of the table by examining its children's lines.
	startLine := -1
	endLine := -1

	// Walk all rows to find line range
	for child := node.FirstChild(); child != nil; child = child.NextSibling() {
		// TableHeader or TableBody
		if child.Type() != ast.TypeBlock {
			continue
		}
		for row := child.FirstChild(); row != nil; row = row.NextSibling() {
			if row.Type() != ast.TypeBlock {
				continue
			}
			if row.Lines().Len() > 0 {
				seg := row.Lines().At(0)
				lineIdx := findLineForOffset(lineOffsets, seg.Start)
				if lineIdx >= 0 {
					if startLine < 0 || lineIdx < startLine {
						startLine = lineIdx
					}
					if lineIdx > endLine {
						endLine = lineIdx
					}
				}
			}
			// Also check cells
			for cell := row.FirstChild(); cell != nil; cell = cell.NextSibling() {
				if cell.Type() != ast.TypeBlock {
					continue
				}
				if cell.Lines().Len() > 0 {
					seg := cell.Lines().At(0)
					lineIdx := findLineForOffset(lineOffsets, seg.Start)
					if lineIdx >= 0 {
						if startLine < 0 || lineIdx < startLine {
							startLine = lineIdx
						}
						if lineIdx > endLine {
							endLine = lineIdx
						}
					}
				}
			}
		}
	}

	if startLine < 0 || endLine < 0 {
		// Fallback: scan from node position
		startLine, endLine = findTableLinesFromNode(node, src, lines, lineOffsets)
		if startLine < 0 {
			return
		}
	}

	// Include the separator line (e.g., |---|---|) which is between header and body
	// Scan from startLine to endLine for table-like lines and extend
	for i := startLine; i <= endLine && i < len(lines); i++ {
		if !isTableLine(lines[i]) {
			endLine = i - 1
			break
		}
	}
	// Extend endLine forward if subsequent lines are still table lines
	for i := endLine + 1; i < len(lines); i++ {
		if isTableLine(lines[i]) {
			endLine = i
		} else {
			break
		}
	}

	if startLine < 0 || endLine < startLine {
		return
	}

	startOff := lineOffsets[startLine]
	endOff := lineOffsets[endLine] + len(lines[endLine])

	// Compute column widths and identify separator line
	colWidths, sepLine := computeTableMetrics(lines, startLine, endLine)

	// Extract alignments from goldmark AST
	alignments := make([]int, len(node.Alignments))
	for i, a := range node.Alignments {
		alignments[i] = int(a)
	}

	*blockID++
	*blocks = append(*blocks, mdBlock{
		kind:       TokenTable,
		id:         *blockID,
		startLine:  startLine,
		endLine:    endLine,
		startOff:   startOff,
		endOff:     endOff,
		colWidths:  colWidths,
		alignments: alignments,
		sepLine:    sepLine,
		headerEnd:  startLine,
	})
}

// findTableLinesFromNode attempts to determine the table's line range from context.
func findTableLinesFromNode(
	node *east.Table,
	src []byte,
	lines []string,
	lineOffsets []int,
) (int, int) {
	// Try to get position from the first child that has segments
	for child := node.FirstChild(); child != nil; child = child.NextSibling() {
		for row := child.FirstChild(); row != nil; row = row.NextSibling() {
			for cell := row.FirstChild(); cell != nil; cell = cell.NextSibling() {
				for inline := cell.FirstChild(); inline != nil; inline = inline.NextSibling() {
					if inline.Kind() == ast.KindText {
						textNode := inline.(*ast.Text)
						seg := textNode.Segment
						lineIdx := findLineForOffset(lineOffsets, seg.Start)
						if lineIdx >= 0 {
							// Found a starting point, now scan for contiguous table lines
							start := lineIdx
							for start > 0 && isTableLine(lines[start-1]) {
								start--
							}
							end := lineIdx
							for end+1 < len(lines) && isTableLine(lines[end+1]) {
								end++
							}
							return start, end
						}
					}
				}
			}
		}
	}
	return -1, -1
}

// isFenceLine checks if a line is a code fence opening/closing marker.
func isFenceLine(line string) bool {
	trimmed := strings.TrimLeft(line, " ")
	if len(trimmed) < 3 {
		return false
	}
	if strings.HasPrefix(trimmed, "```") {
		return true
	}
	if strings.HasPrefix(trimmed, "~~~") {
		return true
	}
	return false
}

// extractFenceLanguage extracts the language identifier from an opening fence line.
func extractFenceLanguage(line string) string {
	trimmed := strings.TrimLeft(line, " ")
	// Strip fence chars
	i := 0
	if strings.HasPrefix(trimmed, "```") {
		i = 3
	} else if strings.HasPrefix(trimmed, "~~~") {
		i = 3
	}
	// Count additional fence chars
	for i < len(trimmed) && trimmed[i] == trimmed[0] {
		i++
	}
	lang := strings.TrimSpace(trimmed[i:])
	// Language stops at first space (per CommonMark)
	if idx := strings.IndexByte(lang, ' '); idx >= 0 {
		lang = lang[:idx]
	}
	return lang
}

// isTableLine checks if a line looks like part of a markdown table.
func isTableLine(line string) bool {
	trimmed := strings.TrimSpace(line)
	if len(trimmed) == 0 {
		return false
	}
	return trimmed[0] == '|' || strings.Contains(trimmed, "|")
}

// findEmptyFenceContentStart locates content start for an empty fenced code block.
func findEmptyFenceContentStart(
	node *ast.FencedCodeBlock,
	src []byte,
	lines []string,
	lineOffsets []int,
) int {
	// For empty code blocks, find the opening fence using sibling context
	if prev := node.PreviousSibling(); prev != nil {
		endOff := findLastOffset(prev)
		if endOff >= 0 {
			lineIdx := findLineForOffset(lineOffsets, endOff)
			// Scan forward from next line for a fence
			for i := lineIdx + 1; i < len(lines); i++ {
				if isFenceLine(lines[i]) {
					// This is the opening fence; "content" starts after it
					if i+1 < len(lines) {
						return lineOffsets[i+1]
					}
					return lineOffsets[i] + len(lines[i]) + 1
				}
			}
		}
	} else {
		// No previous sibling, try from start of document
		for i := 0; i < len(lines); i++ {
			if isFenceLine(lines[i]) {
				if i+1 < len(lines) {
					return lineOffsets[i+1]
				}
				return lineOffsets[i] + len(lines[i]) + 1
			}
		}
	}
	return -1
}

// shouldRevealBlock determines if a block should be revealed based on cursor position.
func shouldRevealBlock(block mdBlock, cursorLine int) bool {
	return cursorLine >= block.startLine && cursorLine <= block.endLine
}

// blockSpansForLine generates SyntaxSpan(s) for a line that belongs to a block.
func blockSpansForLine(
	block mdBlock,
	lineIdx int,
	lineText string,
	lineStart int,
	revealed bool,
	fmMode FrontmatterMode,
	fmError string,
	mdSpans []mdSpan,
	parsed []parsedLine,
	availableWidth int,
) []SyntaxSpan {
	if revealed {
		return []SyntaxSpan{{
			Text:        lineText,
			Kind:        block.kind,
			State:       Revealed,
			BufferStart: lineStart,
			BufferEnd:   lineStart + len(lineText),
			Language:    block.language,
			BlockID:     block.id,
			BlockStart:  block.startOff,
			BlockEnd:    block.endOff,
		}}
	}

	// Rendered mode
	switch block.kind {
	case TokenCodeFence:
		return codeFenceRenderedSpans(block, lineIdx, lineText, lineStart)
	case TokenTable:
		return tableRenderedSpans(block, lineIdx, lineText, lineStart, mdSpans, parsed, availableWidth)
	case TokenFrontmatter:
		return frontmatterRenderedSpans(block, lineIdx, lineText, lineStart, fmMode, fmError)
	case TokenMathBlock:
		return mathBlockRenderedSpans(block, lineIdx, lineText, lineStart)
	default:
		return []SyntaxSpan{{
			Text:        lineText,
			Kind:        block.kind,
			State:       Rendered,
			BufferStart: lineStart,
			BufferEnd:   lineStart + len(lineText),
			BlockID:     block.id,
			BlockStart:  block.startOff,
			BlockEnd:    block.endOff,
		}}
	}
}

// codeFenceRenderedSpans produces spans for a code fence line in rendered mode.
func codeFenceRenderedSpans(block mdBlock, lineIdx int, lineText string, lineStart int) []SyntaxSpan {
	isFenceMarkerLine := lineIdx == block.startLine || lineIdx == block.endLine

	if isFenceMarkerLine {
		// Fence marker lines render as empty in rendered mode
		return []SyntaxSpan{{
			Text:        "",
			Kind:        TokenCodeFence,
			State:       Rendered,
			BufferStart: lineStart,
			BufferEnd:   lineStart + len(lineText),
			Language:    block.language,
			BlockID:     block.id,
			BlockStart:  block.startOff,
			BlockEnd:    block.endOff,
		}}
	}

	// Content lines show their text
	return []SyntaxSpan{{
		Text:        lineText,
		Kind:        TokenCodeFence,
		State:       Rendered,
		BufferStart: lineStart,
		BufferEnd:   lineStart + len(lineText),
		Language:    block.language,
		BlockID:     block.id,
		BlockStart:  block.startOff,
		BlockEnd:    block.endOff,
	}}
}
