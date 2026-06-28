package display

import (
	"github.com/yuin/goldmark/ast"
	east "github.com/yuin/goldmark/extension/ast"
)

// inline_emit.go flattens goldmark's inline AST into buffer-contiguous mdSpans.
//
// The whole rendering pipeline relies on one invariant for every folded span:
//
//	span.text == src[span.start+span.delimLeft : span.end-span.delimRight]
//
// i.e. the visible text is a single CONTIGUOUS source run, with the bytes before
// it (delimLeft) and after it (delimRight) hidden. A token whose visible run sits
// in the middle of its delimiters (wiki [[target|label]]) or whose fallback text
// lives in a hidden region (empty-alt ![](url)) breaks that invariant when handled
// by naive per-node walkers — which is the root cause of the link rendering bugs.
//
// The emitter walks an inline subtree once, accumulating composable decoration
// marks (bold/italic/strike) from ancestor emphasis/strikethrough nodes, and emits
// one span per maximal visible run. Nested formatting (e.g. **[x](y)**) flattens to
// a single span carrying both the content role (Kind=TokenLink) and the marks
// (MarkBold) — never two overlapping spans (which SPAN-COVER forbids).

// emitInline processes one top-level inline token and appends its flattened spans
// to the per-line result. Called from parseMarkdown's ast.Walk for the outermost
// inline token only (the walk returns WalkSkipChildren afterwards), so nested
// tokens are flattened here rather than revisited.
func emitInline(node ast.Node, src []byte, lines []string, lineOffsets []int, result []parsedLine) {
	spans := buildInlineSpans(node, src, 0)
	if len(spans) == 0 {
		return
	}

	// The whole token reveals as a unit: a cursor anywhere in [revealStart,
	// revealEnd) shows raw markup for every sub-span together.
	revealStart := spans[0].start
	revealEnd := spans[len(spans)-1].end

	for _, sp := range spans {
		lineIdx := findLineForOffset(lineOffsets, sp.start)
		if lineIdx < 0 || lineIdx >= len(lines) {
			continue
		}
		ls := lineOffsets[lineIdx]
		// Convert absolute source offsets to line-local (mdSpan.start/end are
		// byte offsets WITHIN the line).
		sp.lineStart = ls
		sp.start -= ls
		sp.end -= ls
		if sp.start < 0 {
			sp.start = 0
		}
		if sp.end > len(lines[lineIdx]) {
			sp.end = len(lines[lineIdx])
		}
		sp.revealStart = revealStart - ls
		sp.revealEnd = revealEnd - ls
		sp.revealSet = true
		result[lineIdx].spans = append(result[lineIdx].spans, sp)
	}
}

// buildInlineSpans flattens an inline node subtree into contiguous mdSpans in
// ABSOLUTE source coordinates (start/end are absolute byte offsets; emitInline
// converts them to line-local). Each returned span's text equals the contiguous
// source run src[start+delimLeft : end-delimRight]. marks carries the decorations
// inherited from ancestor emphasis/strikethrough nodes.
func buildInlineSpans(node ast.Node, src []byte, marks InlineMarks) []mdSpan {
	switch n := node.(type) {
	case *ast.Text:
		return textLeafSpans(n, src, marks)
	case *ast.Emphasis:
		m := marks
		switch n.Level {
		case 1:
			m |= MarkItalic
		case 2:
			m |= MarkBold
		default:
			m |= MarkBold | MarkItalic
		}
		return wrapDelims(buildChildSpans(n, src, m), n.Level, n.Level)
	case *east.Strikethrough:
		return wrapDelims(buildChildSpans(n, src, marks|MarkStrikethrough), 2, 2)
	case *ast.CodeSpan:
		return codeSpanSpans(n, src, marks)
	case *ast.Link:
		return linkSpans(n, src, marks)
	case *ast.Image:
		return imageSpans(n, src, marks)
	case *WikiLinkNode:
		return wikiLinkSpans(n, src, marks)
	default:
		// Unknown inline container — flatten its children with current marks.
		return buildChildSpans(node, src, marks)
	}
}

// buildChildSpans flattens all inline children left-to-right.
func buildChildSpans(node ast.Node, src []byte, marks InlineMarks) []mdSpan {
	var out []mdSpan
	for child := node.FirstChild(); child != nil; child = child.NextSibling() {
		out = append(out, buildInlineSpans(child, src, marks)...)
	}
	return out
}

// textLeafSpans emits one plain-text span for a Text node, trimming a trailing
// soft line break so the text stays within its line.
func textLeafSpans(n *ast.Text, src []byte, marks InlineMarks) []mdSpan {
	start, stop := n.Segment.Start, n.Segment.Stop
	if start < 0 || stop > len(src) || stop <= start {
		return nil
	}
	for stop > start && (src[stop-1] == '\n' || src[stop-1] == '\r') {
		stop--
	}
	if stop <= start {
		return nil
	}
	return []mdSpan{{
		kind:  TokenText,
		marks: marks,
		start: start,
		end:   stop,
		text:  string(src[start:stop]),
	}}
}

// wrapDelims attaches openLen bytes of opening delimiter to the first child span
// and closeLen bytes of closing delimiter to the last, keeping the visible runs
// contiguous. Used by emphasis and strikethrough, whose delimiters sit
// immediately around the content (CommonMark forbids inner padding).
func wrapDelims(spans []mdSpan, openLen, closeLen int) []mdSpan {
	if len(spans) == 0 {
		return spans
	}
	spans[0].start -= openLen
	spans[0].delimLeft += openLen
	last := len(spans) - 1
	spans[last].end += closeLen
	spans[last].delimRight += closeLen
	return spans
}

// codeSpanSpans emits a single InlineCode span. Backtick run width is read from
// the source on each side; any CommonMark-stripped padding space falls into the
// hidden delimiter, preserving the contiguous-middle invariant.
func codeSpanSpans(n *ast.CodeSpan, src []byte, marks InlineMarks) []mdSpan {
	first, last := -1, -1
	for c := n.FirstChild(); c != nil; c = c.NextSibling() {
		if c.Kind() == ast.KindText {
			seg := c.(*ast.Text).Segment
			if first < 0 || seg.Start < first {
				first = seg.Start
			}
			if seg.Stop > last {
				last = seg.Stop
			}
		}
	}
	if first < 0 || last > len(src) {
		return nil
	}
	start := first
	for start > 0 && src[start-1] == '`' {
		start--
	}
	end := last
	for end < len(src) && src[end] == '`' {
		end++
	}
	return []mdSpan{{
		kind:       TokenInlineCode,
		marks:      marks,
		start:      start,
		end:        end,
		text:       string(src[first:last]),
		delimLeft:  first - start,
		delimRight: end - last,
	}}
}

// linkSpans flattens [text](url) (and reference/shortcut links) into spans, then
// folds the opening [ and closing ](url) into the edge spans' delimiters.
func linkSpans(n *ast.Link, src []byte, marks InlineMarks) []mdSpan {
	children := buildChildSpans(n, src, marks)
	if len(children) == 0 {
		return emptyContentSpan(n, src, marks, false, string(n.Destination))
	}
	contentEnd := children[len(children)-1].end
	end := scanLinkClose(src, contentEnd)
	for i := range children {
		// Only promote plain text to the link role; a nested content role
		// (code/image) keeps its own kind (and is not separately navigable).
		if children[i].kind == TokenText {
			children[i].kind = TokenLink
			children[i].linkURL = string(n.Destination)
		}
	}
	children[0].start--
	children[0].delimLeft++
	last := len(children) - 1
	children[last].delimRight += end - children[last].end
	children[last].end = end
	return children
}

// imageSpans flattens ![alt](url). With empty alt, the URL becomes the visible
// label (BUG4 fix) so no fallback text ever lands outside the source it maps to.
func imageSpans(n *ast.Image, src []byte, marks InlineMarks) []mdSpan {
	url := string(n.Destination)
	children := buildChildSpans(n, src, marks)
	if len(children) == 0 {
		return emptyContentSpan(n, src, marks, true, url)
	}
	contentEnd := children[len(children)-1].end
	end := scanLinkClose(src, contentEnd)
	for i := range children {
		children[i].kind = TokenImage
		children[i].linkURL = url
	}
	children[0].start -= 2 // ![
	children[0].delimLeft += 2
	last := len(children) - 1
	children[last].delimRight += end - children[last].end
	children[last].end = end
	return children
}

// wikiLinkSpans emits a single span for [[target|label]] / ![[embed]] using the
// exact offsets the wiki-link parser captured: the visible label is contiguous,
// with [[target| (or ![[target|) hidden on the left and ]] on the right.
func wikiLinkSpans(n *WikiLinkNode, src []byte, marks InlineMarks) []mdSpan {
	if n.TokenStart < 0 || n.TokenStop > len(src) ||
		n.LabelStart < n.TokenStart || n.LabelStop > n.TokenStop || n.LabelStart > n.LabelStop {
		return nil
	}
	return []mdSpan{{
		kind:            TokenWikiLink,
		marks:           marks,
		start:           n.TokenStart,
		end:             n.TokenStop,
		text:            string(src[n.LabelStart:n.LabelStop]),
		delimLeft:       n.LabelStart - n.TokenStart,
		delimRight:      n.TokenStop - n.LabelStop,
		linkURL:         string(n.Target),
		wikiLinkTarget:  string(n.Target),
		wikiLinkLabel:   string(n.Label),
		wikiLinkIsImage: n.Embed,
	}}
}

// emptyContentSpan handles a Link/Image with no visible child text by using the
// destination URL as the label. It verifies the exact []( / ![]( prefix at the
// anchored start and returns nil on mismatch (the region then falls back to raw
// text — a safe degradation, never a scramble).
func emptyContentSpan(n ast.Node, src []byte, marks InlineMarks, isImage bool, url string) []mdSpan {
	start := findInlineStart(n)
	if start < 0 {
		return nil
	}
	kind := TokenLink
	openLen := 1 // [](
	if isImage {
		kind = TokenImage
		openLen = 2 // ![](
	}
	// Verify the literal empty-content prefix: "[]" or "![]" then "(".
	parenOpen := start + openLen + 1 // past ![ / [ and the ]
	if parenOpen >= len(src) {
		return nil
	}
	if isImage {
		if src[start] != '!' || start+1 >= len(src) || src[start+1] != '[' {
			return nil
		}
	} else if src[start] != '[' {
		return nil
	}
	if src[parenOpen-1] != ']' || src[parenOpen] != '(' {
		return nil
	}
	destStart := parenOpen + 1
	destEnd := destStart
	depth := 1
	for destEnd < len(src) && depth > 0 {
		switch src[destEnd] {
		case '(':
			depth++
		case ')':
			depth--
		case '\n':
			return nil
		}
		if depth == 0 {
			break
		}
		destEnd++
	}
	if destEnd >= len(src) || src[destEnd] != ')' {
		return nil
	}
	end := destEnd + 1
	return []mdSpan{{
		kind:       kind,
		marks:      marks,
		start:      start,
		end:        end,
		text:       string(src[destStart:destEnd]),
		delimLeft:  destStart - start,
		delimRight: end - destEnd,
		linkURL:    url,
	}}
}

// scanLinkClose returns the source offset just past a link's closing delimiter,
// starting at the byte after the link text. Handles inline ](dest) with balanced
// parens, and reference/shortcut ][id] / [id] forms.
func scanLinkClose(src []byte, contentEnd int) int {
	end := contentEnd
	if end < len(src) && src[end] == ']' {
		end++
	}
	switch {
	case end < len(src) && src[end] == '(':
		end++
		depth := 1
		for end < len(src) && depth > 0 {
			switch src[end] {
			case '(':
				depth++
			case ')':
				depth--
			}
			end++
		}
	case end < len(src) && src[end] == '[':
		end++
		for end < len(src) && src[end] != ']' {
			end++
		}
		if end < len(src) {
			end++
		}
	}
	return end
}

// findInlineStart anchors a childless inline Link/Image (empty text/alt) at the
// byte where its markup begins: the end of the preceding text sibling, else the
// start of the parent block's first line.
func findInlineStart(n ast.Node) int {
	if prev := n.PreviousSibling(); prev != nil && prev.Kind() == ast.KindText {
		return prev.(*ast.Text).Segment.Stop
	}
	if parent := n.Parent(); parent != nil && parent.Type() == ast.TypeBlock && parent.Lines().Len() > 0 {
		return parent.Lines().At(0).Start
	}
	return -1
}
