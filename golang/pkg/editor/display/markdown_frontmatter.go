package display

import (
	"fmt"
	"strings"

	"gopkg.in/yaml.v3"
)

// parseAdvancedBlocks detects frontmatter and math blocks via regex-style scanning.
// These elements aren't supported by goldmark's default parser without extensions.
func parseAdvancedBlocks(content string, fmMode FrontmatterMode) []mdBlock {
	lines := strings.Split(content, "\n")
	lineOffsets := computeLineOffsets(lines)

	var blocks []mdBlock
	blockID := 1000 // offset to avoid collision with goldmark-based block IDs

	// Frontmatter: must start at line 0 with "---"
	if fmMode != FrontmatterHidden {
		if fmEnd := detectFrontmatter(lines); fmEnd >= 0 {
			blockID++
			blocks = append(blocks, mdBlock{
				kind:      TokenFrontmatter,
				id:        blockID,
				startLine: 0,
				endLine:   fmEnd,
				startOff:  0,
				endOff:    lineOffsets[fmEnd] + len(lines[fmEnd]),
			})
		}
	} else {
		// Hidden mode: still need a block so fence lines are suppressed
		if fmEnd := detectFrontmatter(lines); fmEnd >= 0 {
			blockID++
			blocks = append(blocks, mdBlock{
				kind:      TokenFrontmatter,
				id:        blockID,
				startLine: 0,
				endLine:   fmEnd,
				startOff:  0,
				endOff:    lineOffsets[fmEnd] + len(lines[fmEnd]),
			})
		}
	}

	// Math blocks: "$$" on its own line opens/closes
	blocks = append(blocks, detectMathBlocks(lines, lineOffsets, &blockID)...)

	return blocks
}

// detectFrontmatter checks if the document starts with YAML frontmatter.
// Returns the closing "---" line index, or -1 if no frontmatter found.
func detectFrontmatter(lines []string) int {
	if len(lines) < 3 {
		return -1
	}
	if strings.TrimSpace(lines[0]) != "---" {
		return -1
	}
	for i := 1; i < len(lines); i++ {
		if strings.TrimSpace(lines[i]) == "---" {
			return i
		}
	}
	return -1
}

// detectMathBlocks finds $$ ... $$ block math regions.
func detectMathBlocks(lines []string, lineOffsets []int, blockID *int) []mdBlock {
	var blocks []mdBlock
	i := 0
	for i < len(lines) {
		if isMathBlockDelimiter(lines[i]) {
			// Find closing $$
			closeIdx := -1
			for j := i + 1; j < len(lines); j++ {
				if isMathBlockDelimiter(lines[j]) {
					closeIdx = j
					break
				}
			}
			if closeIdx > 0 {
				*blockID++
				startOff := lineOffsets[i]
				endOff := lineOffsets[closeIdx] + len(lines[closeIdx])
				blocks = append(blocks, mdBlock{
					kind:      TokenMathBlock,
					id:        *blockID,
					startLine: i,
					endLine:   closeIdx,
					startOff:  startOff,
					endOff:    endOff,
				})
				i = closeIdx + 1
				continue
			}
		}
		i++
	}
	return blocks
}

// isMathBlockDelimiter checks if a line is a $$ delimiter.
func isMathBlockDelimiter(line string) bool {
	trimmed := strings.TrimSpace(line)
	return trimmed == "$$"
}

// computeLineOffsets returns the byte offset of each line start.
func computeLineOffsets(lines []string) []int {
	offsets := make([]int, len(lines))
	offset := 0
	for i, line := range lines {
		offsets[i] = offset
		offset += len(line) + 1
	}
	return offsets
}

// safeYAMLUnmarshal is the single chokepoint for all YAML parsing in the
// display package. yaml.v3 can panic on certain malformed inputs (e.g. merge
// keys whose value contains unhashable slice types). The recover catches
// that panic and returns it as a proper error so the editor never crashes
// (§1.3).
func safeYAMLUnmarshal(data []byte, target any) (err error) {
	defer func() {
		if v := recover(); v != nil {
			err = fmt.Errorf("yaml parse panic: %v", v)
		}
	}()
	return yaml.Unmarshal(data, target)
}

// parseFrontmatterYAML parses the YAML body between the --- delimiters.
// lines is the full document line slice; fmEnd is the index returned by detectFrontmatter.
// Returns (nil, nil) for an empty frontmatter body.
func parseFrontmatterYAML(lines []string, fmEnd int) (map[string]any, error) {
	if fmEnd <= 1 {
		return nil, nil
	}
	body := strings.Join(lines[1:fmEnd], "\n")
	if strings.TrimSpace(body) == "" {
		return nil, nil
	}
	var out map[string]any
	if err := safeYAMLUnmarshal([]byte(body), &out); err != nil {
		return nil, err
	}
	return out, nil
}

// frontmatterRenderedSpans produces spans for frontmatter lines in rendered
// mode. fmError is kept as error (not downgraded to a string upstream) all
// the way to this render boundary — the ONLY place its value is consulted —
// per §1.3/CLAUDE.md "keep errors as error, call .Error() only at the
// display boundary." Today that boundary never actually renders the message
// text (only its presence, for the "(invalid YAML)" label); a future
// tooltip/status hint can call fmError.Error() here without threading a new
// parameter back through syncInternal.
func frontmatterRenderedSpans(
	block mdBlock, lineIdx int, lineText string, lineStart int, fmMode FrontmatterMode, fmError error,
) []SyntaxSpan {
	switch fmMode {
	case FrontmatterHidden:
		// All lines render as empty
		return []SyntaxSpan{{
			Text:        "",
			Kind:        TokenFrontmatter,
			State:       Rendered,
			BufferStart: lineStart,
			BufferEnd:   lineStart + len(lineText),
			BlockID:     block.id,
			BlockStart:  block.startOff,
			BlockEnd:    block.endOff,
		}}
	case FrontmatterSource:
		// Show as-is but with semantic token kind
		return []SyntaxSpan{{
			Text:        lineText,
			Kind:        TokenFrontmatter,
			State:       Rendered,
			BufferStart: lineStart,
			BufferEnd:   lineStart + len(lineText),
			BlockID:     block.id,
			BlockStart:  block.startOff,
			BlockEnd:    block.endOff,
		}}
	default: // FrontmatterCollapsed
		// Fence lines (first and last) show a collapsed indicator on first line only
		if lineIdx == block.startLine {
			label := "··· frontmatter ···"
			if fmError != nil {
				label = "··· frontmatter (invalid YAML) ···"
			}
			return []SyntaxSpan{{
				Text:        label,
				Kind:        TokenFrontmatter,
				State:       Rendered,
				BufferStart: lineStart,
				BufferEnd:   lineStart + len(lineText),
				BlockID:     block.id,
				BlockStart:  block.startOff,
				BlockEnd:    block.endOff,
			}}
		}
		// All other lines render as empty in collapsed mode
		return []SyntaxSpan{{
			Text:        "",
			Kind:        TokenFrontmatter,
			State:       Rendered,
			BufferStart: lineStart,
			BufferEnd:   lineStart + len(lineText),
			BlockID:     block.id,
			BlockStart:  block.startOff,
			BlockEnd:    block.endOff,
		}}
	}
}

// mathBlockRenderedSpans produces spans for math block lines in rendered mode.
func mathBlockRenderedSpans(block mdBlock, lineIdx int, lineText string, lineStart int) []SyntaxSpan {
	isDelimiterLine := lineIdx == block.startLine || lineIdx == block.endLine

	if isDelimiterLine {
		// $$ delimiter lines render as empty
		return []SyntaxSpan{{
			Text:        "",
			Kind:        TokenMathBlock,
			State:       Rendered,
			BufferStart: lineStart,
			BufferEnd:   lineStart + len(lineText),
			BlockID:     block.id,
			BlockStart:  block.startOff,
			BlockEnd:    block.endOff,
		}}
	}

	// Content lines show their math source as semantic text
	return []SyntaxSpan{{
		Text:        lineText,
		Kind:        TokenMathBlock,
		State:       Rendered,
		BufferStart: lineStart,
		BufferEnd:   lineStart + len(lineText),
		BlockID:     block.id,
		BlockStart:  block.startOff,
		BlockEnd:    block.endOff,
	}}
}
