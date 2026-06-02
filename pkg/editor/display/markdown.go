package display

import (
	"strings"

	"github.com/yuin/goldmark"
	"github.com/yuin/goldmark/ast"
	"github.com/yuin/goldmark/extension"
	east "github.com/yuin/goldmark/extension/ast"
	"github.com/yuin/goldmark/parser"
	"github.com/yuin/goldmark/text"
)

// mdParser is a package-level goldmark instance configured with extensions.
var mdParser parser.Parser

func init() {
	md := goldmark.New(
		goldmark.WithExtensions(
			extension.Strikethrough,
			extension.TaskList,
			extension.Table,
			WikiLinkExtension,
		),
	)
	mdParser = md.Parser()
}

// mdSpan represents a parsed markdown element with byte ranges within a line.
type mdSpan struct {
	kind       TokenKind
	lineStart  int // byte offset of the line within the full document
	start      int // start byte within the line
	end        int // end byte within the line
	text       string
	delimLeft  int // bytes of left delimiter to hide
	delimRight int // bytes of right delimiter to hide
	linkURL    string
	level      int // heading level (1-6)
	// Wiki link metadata (set for TokenWikiLink spans)
	wikiLinkTarget  string // resolved file path for wiki links
	wikiLinkLabel   string // display text for wiki links
	wikiLinkIsImage bool   // true for embedded images ![[image.png]]
}

// parsedLine holds the spans extracted for a single line.
type parsedLine struct {
	spans []mdSpan
}

// parseMarkdown parses the full document and returns per-line span info and blocks.
func parseMarkdown(content string) ([]parsedLine, []mdBlock) {
	src := []byte(content)
	reader := text.NewReader(src)
	tree := mdParser.Parse(reader)

	lines := strings.Split(content, "\n")
	result := make([]parsedLine, len(lines))

	// Compute line start offsets
	lineOffsets := make([]int, len(lines))
	offset := 0
	for i, line := range lines {
		lineOffsets[i] = offset
		offset += len(line) + 1 // +1 for newline
	}

	var blocks []mdBlock
	blockID := 0

	// Walk the AST and extract inline elements and blocks
	ast.Walk(tree, func(n ast.Node, entering bool) (ast.WalkStatus, error) {
		if !entering {
			return ast.WalkContinue, nil
		}

		switch node := n.(type) {
		case *ast.Heading:
			walkHeading(node, src, lines, lineOffsets, result)
		case *ast.Emphasis:
			walkEmphasis(node, src, lines, lineOffsets, result)
		case *east.Strikethrough:
			walkStrikethrough(node, src, lines, lineOffsets, result)
		case *ast.CodeSpan:
			walkCodeSpan(node, src, lines, lineOffsets, result)
		case *ast.Link:
			walkLink(node, src, lines, lineOffsets, result)
		case *ast.Blockquote:
			walkBlockquote(node, src, lines, lineOffsets, result)
		case *ast.ThematicBreak:
			walkThematicBreak(node, src, lines, lineOffsets, result)
		case *ast.ListItem:
			walkTaskList(node, src, lines, lineOffsets, result)
		case *ast.FencedCodeBlock:
			walkFencedCodeBlock(node, src, lines, lineOffsets, &blockID, &blocks)
			return ast.WalkSkipChildren, nil
		case *east.Table:
			walkTable(node, src, lines, lineOffsets, result, &blockID, &blocks)
			// Do NOT skip children — let inline walkers visit cell content for rich rendering
		case *WikiLinkNode:
			walkWikiLink(node, src, lines, lineOffsets, result)
		case *ast.Image:
			walkImage(node, src, lines, lineOffsets, result)
			return ast.WalkSkipChildren, nil
		}

		return ast.WalkContinue, nil
	})

	return result, blocks
}

// parseMarkdownAdvanced wraps parseMarkdown and adds advanced inline parsing.
func parseMarkdownAdvanced(content string) ([]parsedLine, []mdBlock) {
	parsed, blocks := parseMarkdown(content)
	parsed = parseAdvancedInlines(content, parsed)
	return parsed, blocks
}

// findLineForOffset returns the line index that contains the given byte offset.
func findLineForOffset(lineOffsets []int, offset int) int {
	lo, hi := 0, len(lineOffsets)-1
	for lo <= hi {
		mid := (lo + hi) / 2
		if lineOffsets[mid] <= offset {
			lo = mid + 1
		} else {
			hi = mid - 1
		}
	}
	return hi
}
