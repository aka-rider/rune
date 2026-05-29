package display

import (
	"bytes"
	"strings"

	"github.com/yuin/goldmark/ast"
	east "github.com/yuin/goldmark/extension/ast"
	"github.com/yuin/goldmark/text"
)

func walkHeading(node *ast.Heading, src []byte, lines []string, lineOffsets []int, result []parsedLine) {
	if node.Lines().Len() == 0 {
		return
	}
	seg := node.Lines().At(0)
	lineIdx := findLineForOffset(lineOffsets, seg.Start)
	if lineIdx < 0 || lineIdx >= len(lines) {
		return
	}

	line := lines[lineIdx]
	hashes := 0
	for hashes < len(line) && line[hashes] == '#' {
		hashes++
	}
	if hashes == 0 || hashes > 6 {
		return
	}
	delimLen := hashes
	if delimLen < len(line) && line[delimLen] == ' ' {
		delimLen++
	}

	result[lineIdx].spans = append(result[lineIdx].spans, mdSpan{
		kind:       TokenHeading,
		lineStart:  lineOffsets[lineIdx],
		start:      0,
		end:        len(line),
		text:       line[delimLen:],
		delimLeft:  delimLen,
		delimRight: 0,
		level:      hashes,
	})
}

func walkEmphasis(node *ast.Emphasis, src []byte, lines []string, lineOffsets []int, result []parsedLine) {
	seg := spanSegment(node, src)
	if seg.start < 0 {
		return
	}

	lineIdx := findLineForOffset(lineOffsets, seg.start)
	if lineIdx < 0 || lineIdx >= len(lines) {
		return
	}

	localStart := seg.start - lineOffsets[lineIdx]
	localEnd := seg.end - lineOffsets[lineIdx]
	if localEnd > len(lines[lineIdx]) {
		localEnd = len(lines[lineIdx])
	}
	if localStart < 0 {
		localStart = 0
	}

	delimSize := node.Level
	kind := TokenItalic
	if node.Level >= 2 {
		kind = TokenBold
	}

	innerText := extractChildText(node, src)

	result[lineIdx].spans = append(result[lineIdx].spans, mdSpan{
		kind:       kind,
		lineStart:  lineOffsets[lineIdx],
		start:      localStart,
		end:        localEnd,
		text:       innerText,
		delimLeft:  delimSize,
		delimRight: delimSize,
	})
}

func walkStrikethrough(node *east.Strikethrough, src []byte, lines []string, lineOffsets []int, result []parsedLine) {
	seg := spanSegment(node, src)
	if seg.start < 0 {
		return
	}

	lineIdx := findLineForOffset(lineOffsets, seg.start)
	if lineIdx < 0 || lineIdx >= len(lines) {
		return
	}

	localStart := seg.start - lineOffsets[lineIdx]
	localEnd := seg.end - lineOffsets[lineIdx]
	if localEnd > len(lines[lineIdx]) {
		localEnd = len(lines[lineIdx])
	}
	if localStart < 0 {
		localStart = 0
	}

	innerText := extractChildText(node, src)

	result[lineIdx].spans = append(result[lineIdx].spans, mdSpan{
		kind:       TokenStrikethrough,
		lineStart:  lineOffsets[lineIdx],
		start:      localStart,
		end:        localEnd,
		text:       innerText,
		delimLeft:  2,
		delimRight: 2,
	})
}

func walkCodeSpan(node *ast.CodeSpan, src []byte, lines []string, lineOffsets []int, result []parsedLine) {
	seg := spanSegment(node, src)
	if seg.start < 0 {
		return
	}

	lineIdx := findLineForOffset(lineOffsets, seg.start)
	if lineIdx < 0 || lineIdx >= len(lines) {
		return
	}

	localStart := seg.start - lineOffsets[lineIdx]
	localEnd := seg.end - lineOffsets[lineIdx]
	if localEnd > len(lines[lineIdx]) {
		localEnd = len(lines[lineIdx])
	}
	if localStart < 0 {
		localStart = 0
	}

	rawText := lines[lineIdx][localStart:localEnd]
	backticks := 0
	for backticks < len(rawText) && rawText[backticks] == '`' {
		backticks++
	}

	innerText := extractChildText(node, src)

	result[lineIdx].spans = append(result[lineIdx].spans, mdSpan{
		kind:       TokenInlineCode,
		lineStart:  lineOffsets[lineIdx],
		start:      localStart,
		end:        localEnd,
		text:       innerText,
		delimLeft:  backticks,
		delimRight: backticks,
	})
}

func walkLink(node *ast.Link, src []byte, lines []string, lineOffsets []int, result []parsedLine) {
	seg := spanSegment(node, src)
	if seg.start < 0 {
		return
	}

	lineIdx := findLineForOffset(lineOffsets, seg.start)
	if lineIdx < 0 || lineIdx >= len(lines) {
		return
	}

	localStart := seg.start - lineOffsets[lineIdx]
	localEnd := seg.end - lineOffsets[lineIdx]
	if localEnd > len(lines[lineIdx]) {
		localEnd = len(lines[lineIdx])
	}
	if localStart < 0 {
		localStart = 0
	}

	linkText := extractChildText(node, src)

	result[lineIdx].spans = append(result[lineIdx].spans, mdSpan{
		kind:       TokenLink,
		lineStart:  lineOffsets[lineIdx],
		start:      localStart,
		end:        localEnd,
		text:       linkText,
		delimLeft:  1,
		delimRight: localEnd - localStart - 1 - len(linkText),
		linkURL:    string(node.Destination),
	})
}

func walkWikiLink(node *WikiLinkNode, src []byte, lines []string, lineOffsets []int, result []parsedLine) {
	seg := spanSegment(node, src)
	if seg.start < 0 {
		return
	}

	lineIdx := findLineForOffset(lineOffsets, seg.start)
	if lineIdx < 0 || lineIdx >= len(lines) {
		return
	}

	localStart := seg.start - lineOffsets[lineIdx]
	localEnd := seg.end - lineOffsets[lineIdx]
	if localEnd > len(lines[lineIdx]) {
		localEnd = len(lines[lineIdx])
	}
	if localStart < 0 {
		localStart = 0
	}

	linkText := extractChildText(node, src)

	delimLeft := 2
	if node.Embed {
		delimLeft = 3
	}

	result[lineIdx].spans = append(result[lineIdx].spans, mdSpan{
		kind:            TokenWikiLink,
		lineStart:       lineOffsets[lineIdx],
		start:           localStart,
		end:             localEnd,
		text:            linkText,
		delimLeft:       delimLeft,
		delimRight:      2,
		linkURL:         string(node.Target),
		wikiLinkTarget:  string(node.Target),
		wikiLinkLabel:   string(node.Label),
		wikiLinkIsImage: node.Embed,
	})
}

func walkBlockquote(node *ast.Blockquote, src []byte, lines []string, lineOffsets []int, result []parsedLine) {
	for child := node.FirstChild(); child != nil; child = child.NextSibling() {
		if child.Lines().Len() == 0 {
			continue
		}
		for i := 0; i < child.Lines().Len(); i++ {
			seg := child.Lines().At(i)
			lineIdx := findLineForOffset(lineOffsets, seg.Start)
			if lineIdx < 0 || lineIdx >= len(lines) {
				continue
			}

			line := lines[lineIdx]
			trimmed := strings.TrimLeft(line, " ")
			if len(trimmed) == 0 || trimmed[0] != '>' {
				continue
			}

			hasBlockquote := false
			for _, s := range result[lineIdx].spans {
				if s.kind == TokenBlockquote {
					hasBlockquote = true
					break
				}
			}
			if hasBlockquote {
				continue
			}

			delimLen := len(line) - len(trimmed) + 1
			if delimLen < len(line) && line[delimLen] == ' ' {
				delimLen++
			}

			result[lineIdx].spans = append(result[lineIdx].spans, mdSpan{
				kind:       TokenBlockquote,
				lineStart:  lineOffsets[lineIdx],
				start:      0,
				end:        len(line),
				text:       line[delimLen:],
				delimLeft:  delimLen,
				delimRight: 0,
			})
		}
	}
}

func walkThematicBreak(node *ast.ThematicBreak, src []byte, lines []string, lineOffsets []int, result []parsedLine) {
	if node.Lines().Len() == 0 {
		// ThematicBreak often has no lines in goldmark. Determine position
		// by finding the line using sibling/parent context.
		lineIdx := findThematicBreakLine(node, src, lines, lineOffsets)
		if lineIdx < 0 || lineIdx >= len(lines) {
			return
		}
		result[lineIdx].spans = append(result[lineIdx].spans, mdSpan{
			kind:       TokenHorizontalRule,
			lineStart:  lineOffsets[lineIdx],
			start:      0,
			end:        len(lines[lineIdx]),
			text:       "───",
			delimLeft:  len(lines[lineIdx]),
			delimRight: 0,
		})
		return
	}
	seg := node.Lines().At(0)
	lineIdx := findLineForOffset(lineOffsets, seg.Start)
	if lineIdx < 0 || lineIdx >= len(lines) {
		return
	}
	result[lineIdx].spans = append(result[lineIdx].spans, mdSpan{
		kind:       TokenHorizontalRule,
		lineStart:  lineOffsets[lineIdx],
		start:      0,
		end:        len(lines[lineIdx]),
		text:       "───",
		delimLeft:  len(lines[lineIdx]),
		delimRight: 0,
	})
}

// findThematicBreakLine determines which line a ThematicBreak occupies
// by examining sibling nodes and scanning for HR patterns.
func findThematicBreakLine(node ast.Node, src []byte, lines []string, lineOffsets []int) int {
	// Find the end of the previous sibling to determine start search line
	startLine := 0
	if prev := node.PreviousSibling(); prev != nil {
		if prev.Lines().Len() > 0 {
			lastSeg := prev.Lines().At(prev.Lines().Len() - 1)
			startLine = findLineForOffset(lineOffsets, lastSeg.Stop-1) + 1
		} else {
			// Try children of previous sibling
			endOffset := findLastOffset(prev)
			if endOffset >= 0 {
				startLine = findLineForOffset(lineOffsets, endOffset) + 1
			}
		}
	}

	// Scan from startLine for a thematic break pattern
	for i := startLine; i < len(lines); i++ {
		if isThematicBreakLine(lines[i]) {
			return i
		}
	}
	return -1
}

// findLastOffset recursively finds the last byte offset in a node tree.
func findLastOffset(node ast.Node) int {
	if node.Lines().Len() > 0 {
		return node.Lines().At(node.Lines().Len()-1).Stop - 1
	}
	// Check children from last to first
	for child := node.LastChild(); child != nil; child = child.PreviousSibling() {
		if child.Type() == ast.TypeInline {
			continue
		}
		off := findLastOffset(child)
		if off >= 0 {
			return off
		}
	}
	return -1
}

// isThematicBreakLine checks if a line matches thematic break syntax.
func isThematicBreakLine(line string) bool {
	trimmed := strings.TrimSpace(line)
	if len(trimmed) < 3 {
		return false
	}
	ch := trimmed[0]
	if ch != '-' && ch != '*' && ch != '_' {
		return false
	}
	count := 0
	for _, c := range trimmed {
		if c == rune(ch) {
			count++
		} else if c != ' ' {
			return false
		}
	}
	return count >= 3
}

func walkTaskList(node *ast.ListItem, src []byte, lines []string, lineOffsets []int, result []parsedLine) {
	// ListItem doesn't carry lines directly; get from first child (TextBlock/Paragraph)
	var seg text.Segment
	found := false
	if node.Lines().Len() > 0 {
		seg = node.Lines().At(0)
		found = true
	} else {
		for child := node.FirstChild(); child != nil; child = child.NextSibling() {
			if child.Type() != ast.TypeInline && child.Lines().Len() > 0 {
				seg = child.Lines().At(0)
				found = true
				break
			}
		}
	}
	if !found {
		return
	}

	// The segment starts AFTER the list marker ("- "), so back up to find the full line
	lineIdx := findLineForOffset(lineOffsets, seg.Start)
	if lineIdx < 0 || lineIdx >= len(lines) {
		return
	}

	line := lines[lineIdx]
	checkIdx := strings.Index(line, "[ ]")
	glyph := "☐ "
	if checkIdx < 0 {
		checkIdx = strings.Index(line, "[x]")
		if checkIdx < 0 {
			checkIdx = strings.Index(line, "[X]")
		}
		if checkIdx < 0 {
			return
		}
		glyph = "☑ "
	}

	checkEnd := checkIdx + 3
	if checkEnd < len(line) && line[checkEnd] == ' ' {
		checkEnd++
	}

	result[lineIdx].spans = append(result[lineIdx].spans, mdSpan{
		kind:       TokenTaskList,
		lineStart:  lineOffsets[lineIdx],
		start:      checkIdx,
		end:        checkEnd,
		text:       glyph,
		delimLeft:  checkEnd - checkIdx,
		delimRight: 0,
	})
}

// spanSegment gets the byte range of an inline node including delimiters.
func spanSegment(node ast.Node, src []byte) struct{ start, end int } {
	if node.Type() == ast.TypeInline {
		first := -1
		last := -1
		for child := node.FirstChild(); child != nil; child = child.NextSibling() {
			if child.Kind() == ast.KindText {
				textNode := child.(*ast.Text)
				seg := textNode.Segment
				if first < 0 || seg.Start < first {
					first = seg.Start
				}
				if seg.Stop > last {
					last = seg.Stop
				}
			}
		}
		if first >= 0 {
			return expandForDelimiters(node, src, first, last)
		}
	}
	return struct{ start, end int }{-1, -1}
}

// expandForDelimiters finds the actual start/end including delimiters.
func expandForDelimiters(node ast.Node, src []byte, contentStart, contentEnd int) struct{ start, end int } {
	switch n := node.(type) {
	case *ast.Emphasis:
		delim := n.Level
		start := contentStart - delim
		end := contentEnd + delim
		if start < 0 {
			start = 0
		}
		if end > len(src) {
			end = len(src)
		}
		return struct{ start, end int }{start, end}
	case *east.Strikethrough:
		start := contentStart - 2
		end := contentEnd + 2
		if start < 0 {
			start = 0
		}
		if end > len(src) {
			end = len(src)
		}
		return struct{ start, end int }{start, end}
	case *ast.CodeSpan:
		start := contentStart - 1
		for start > 0 && src[start-1] == '`' {
			start--
		}
		backticks := contentStart - start
		end := contentEnd + backticks
		return struct{ start, end int }{start, end}
	case *ast.Link:
		start := contentStart - 1
		if start < 0 {
			start = 0
		}
		end := contentEnd + 1
		if end < len(src) && src[end] == '(' {
			end++
			for end < len(src) && src[end] != ')' {
				end++
			}
			if end < len(src) {
				end++
			}
		}
		return struct{ start, end int }{start, end}
	case *WikiLinkNode:
		start := contentStart - 2
		if n.Embed {
			start = contentStart - 3
		}
		if start < 0 {
			start = 0
		}
		end := contentEnd + 2
		if end > len(src) {
			end = len(src)
		}
		return struct{ start, end int }{start, end}
	}
	return struct{ start, end int }{contentStart, contentEnd}
}

// nodeSegment gets byte range from a block node's lines.
func nodeSegment(node ast.Node) struct{ start, end int } {
	if node.Lines().Len() > 0 {
		first := node.Lines().At(0)
		last := node.Lines().At(node.Lines().Len() - 1)
		return struct{ start, end int }{first.Start, last.Stop}
	}
	return struct{ start, end int }{-1, -1}
}

// extractChildText extracts the visible text content from a node's children.
func extractChildText(node ast.Node, src []byte) string {
	var buf bytes.Buffer
	for child := node.FirstChild(); child != nil; child = child.NextSibling() {
		if child.Kind() == ast.KindText {
			textNode := child.(*ast.Text)
			buf.Write(textNode.Segment.Value(src))
		} else if child.Type() == ast.TypeInline {
			buf.WriteString(extractChildText(child, src))
		}
	}
	return buf.String()
}
