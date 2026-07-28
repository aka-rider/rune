package display

import (
	"strings"
)

// imageExtensions are file extensions that indicate an image file.
var imageExtensions = map[string]bool{
	".png":   true,
	".jpg":   true,
	".jpeg":  true,
	".gif":   true,
	".webp":  true,
	".svg":   true,
	".bmp":   true,
	".apng":  true,
	".avif":  true,
	".jfif":  true,
	".pjpeg": true,
	".pjp":   true,
}

// parseAdvancedInlines scans lines for advanced inline markdown elements
// not handled by goldmark's default parser: inline math, highlights, embed refs, callouts, images.
func parseAdvancedInlines(content string, parsed []parsedLine) []parsedLine {
	lines := strings.Split(content, "\n")
	for i, line := range lines {
		if i >= len(parsed) {
			break
		}
		parseInlineMath(line, i, &parsed[i])
		parseHighlights(line, i, &parsed[i])
		parseCallout(line, i, &parsed[i])
		parseMarkdownImagesWithSpaces(line, i, &parsed[i])
	}
	return parsed
}

// parseInlineMath finds $...$ inline math spans in a line.
// Avoids matching $$ (block delimiters) and escaped dollars.
func parseInlineMath(line string, lineIdx int, pl *parsedLine) {
	i := 0
	for i < len(line) {
		// Skip escaped dollars
		if i > 0 && line[i-1] == '\\' {
			i++
			continue
		}
		if line[i] != '$' {
			i++
			continue
		}
		// Skip $$ (block delimiter)
		if i+1 < len(line) && line[i+1] == '$' {
			i += 2
			continue
		}
		// Found opening $, find closing $
		start := i
		i++
		if i >= len(line) {
			break
		}
		// Content must not start with space
		if line[i] == ' ' {
			continue
		}
		// Find closing $
		closeIdx := -1
		for j := i; j < len(line); j++ {
			if line[j] == '\\' {
				j++ // skip escaped char
				continue
			}
			if line[j] == '$' {
				// Content must not end with space
				if j > i && line[j-1] != ' ' {
					closeIdx = j
					break
				}
			}
		}
		if closeIdx < 0 {
			continue
		}
		// Check overlap with existing spans
		if overlapsExisting(pl.spans, start, closeIdx+1) {
			i = closeIdx + 1
			continue
		}
		innerText := line[start+1 : closeIdx]
		pl.spans = append(pl.spans, mdSpan{
			kind:       TokenInlineMath,
			start:      start,
			end:        closeIdx + 1,
			text:       innerText,
			delimLeft:  1,
			delimRight: 1,
		})
		i = closeIdx + 1
	}
}

// parseHighlights finds ==text== highlight spans in a line.
func parseHighlights(line string, lineIdx int, pl *parsedLine) {
	i := 0
	for i < len(line)-3 { // minimum ==x==
		if line[i] != '=' || i+1 >= len(line) || line[i+1] != '=' {
			i++
			continue
		}
		// Found opening ==
		start := i
		i += 2
		// Find closing ==
		closeIdx := -1
		for j := i + 1; j < len(line)-1; j++ {
			if line[j] == '=' && line[j+1] == '=' {
				closeIdx = j
				break
			}
		}
		if closeIdx < 0 {
			continue
		}
		end := closeIdx + 2
		if overlapsExisting(pl.spans, start, end) {
			i = end
			continue
		}
		innerText := line[start+2 : closeIdx]
		pl.spans = append(pl.spans, mdSpan{
			kind:       TokenHighlight,
			start:      start,
			end:        end,
			text:       innerText,
			delimLeft:  2,
			delimRight: 2,
		})
		i = end
	}
}

// parseCallout detects > [!type] callout syntax in blockquote lines.
// This enriches existing blockquote spans with callout metadata.
func parseCallout(line string, lineIdx int, pl *parsedLine) {
	trimmed := strings.TrimLeft(line, " ")
	if len(trimmed) == 0 || trimmed[0] != '>' {
		return
	}
	// Look for [!type] pattern after >
	after := strings.TrimLeft(trimmed[1:], " ")
	if !strings.HasPrefix(after, "[!") {
		return
	}
	closeIdx := strings.Index(after, "]")
	if closeIdx < 0 {
		return
	}
	calloutKind := after[2:closeIdx]
	if calloutKind == "" {
		return
	}

	// Compute the full callout title span
	// The delimiter includes everything up to and including the ] and optional space
	prefixLen := len(line) - len(trimmed) // leading spaces
	afterBracket := prefixLen + 1         // after >
	afterBracket += len(trimmed[1:]) - len(after)
	titleEnd := afterBracket + closeIdx + 1
	if titleEnd < len(line) && line[titleEnd] == ' ' {
		titleEnd++
	}

	// Determine display text: callout type as title, rest of line as content
	var displayText string
	if titleEnd < len(line) {
		displayText = line[titleEnd:]
	}

	// Replace existing blockquote span if present, or add callout span
	for idx, s := range pl.spans {
		if s.kind == TokenBlockquote {
			pl.spans[idx] = mdSpan{
				kind:       TokenCallout,
				start:      0,
				end:        len(line),
				text:       displayText,
				delimLeft:  titleEnd,
				delimRight: 0,
				linkURL:    calloutKind,
			}
			return
		}
	}

	// No blockquote span found, add callout directly
	pl.spans = append(pl.spans, mdSpan{
		kind:       TokenCallout,
		start:      0,
		end:        len(line),
		text:       displayText,
		delimLeft:  titleEnd,
		delimRight: 0,
		linkURL:    calloutKind,
	})
}

// overlapsExisting checks if a span range [start, end) overlaps any existing span.
func overlapsExisting(spans []mdSpan, start, end int) bool {
	for _, s := range spans {
		if start < s.end && end > s.start {
			return true
		}
	}
	return false
}

// parseMarkdownImagesWithSpaces is a fallback parser for markdown image syntax
// with unescaped spaces in the destination: ![alt](path with spaces.ext).
// Goldmark does not parse these as images. We conservatively require an image
// file extension to avoid false positives.
func parseMarkdownImagesWithSpaces(line string, lineIdx int, pl *parsedLine) {
	i := 0
	for i < len(line) {
		// Find ![
		idx := strings.Index(line[i:], "![")
		if idx < 0 {
			break
		}
		bangPos := i + idx
		// Find closing ]
		altStart := bangPos + 2
		altEnd := strings.Index(line[altStart:], "]")
		if altEnd < 0 {
			i = altStart
			continue
		}
		altEnd += altStart
		// Expect ( immediately after ]
		if altEnd+1 >= len(line) || line[altEnd+1] != '(' {
			i = altEnd + 1
			continue
		}
		destStart := altEnd + 2
		// Find closing ) — does not allow nested parens
		destEnd := strings.Index(line[destStart:], ")")
		if destEnd < 0 {
			i = destStart
			continue
		}
		destEnd += destStart
		dest := line[destStart:destEnd]
		// Require non-empty destination with an image extension.
		if dest == "" {
			i = destEnd + 1
			continue
		}
		if !hasImageExtension(dest) {
			i = destEnd + 1
			continue
		}
		spanStart := bangPos
		spanEnd := destEnd + 1
		// Skip if goldmark or another parser already produced a span here.
		if overlapsExisting(pl.spans, spanStart, spanEnd) {
			i = spanEnd
			continue
		}
		altText := line[altStart:altEnd]
		displayText := altText
		if displayText == "" {
			displayText = dest
		}
		pl.spans = append(pl.spans, mdSpan{
			kind:       TokenImage,
			start:      spanStart,
			end:        spanEnd,
			text:       displayText,
			delimLeft:  2,                                             // ![
			delimRight: spanEnd - spanStart - 2 - (altEnd - altStart), // ](dest)
			linkURL:    dest,
		})
		i = spanEnd
	}
}

// hasImageExtension checks if a path ends with a known image file extension.
func hasImageExtension(path string) bool {
	lower := strings.ToLower(path)
	for ext := range imageExtensions {
		if strings.HasSuffix(lower, ext) {
			return true
		}
	}
	return false
}
