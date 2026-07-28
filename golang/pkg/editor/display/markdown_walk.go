package display

import (
	"strings"

	"github.com/yuin/goldmark/ast"
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
