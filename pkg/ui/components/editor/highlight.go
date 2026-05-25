package editor

import (
	"strings"

	"github.com/alecthomas/chroma/v2"
	"github.com/alecthomas/chroma/v2/lexers"
)

// HighlightSpan represents a single token with its semantic class.
type HighlightSpan struct {
	Text  string
	Class string // e.g. "keyword", "string", "comment", "plain"
}

// CodeHighlighter tokenizes source code for a given language into styled spans.
// Returns nil spans with nil error to indicate unknown language / fallback.
type CodeHighlighter func(language, source string) ([]HighlightSpan, error)

// ChromaHighlighter returns a CodeHighlighter backed by the chroma library.
func ChromaHighlighter() CodeHighlighter {
	return func(language, source string) ([]HighlightSpan, error) {
		lexer := lexers.Get(language)
		if lexer == nil {
			return nil, nil
		}
		lexer = chroma.Coalesce(lexer)

		iter, err := lexer.Tokenise(nil, source)
		if err != nil {
			return nil, err
		}

		var spans []HighlightSpan
		for _, tok := range iter.Tokens() {
			if tok.Value == "" {
				continue
			}
			spans = append(spans, HighlightSpan{
				Text:  tok.Value,
				Class: tokenTypeToClass(tok.Type),
			})
		}
		return spans, nil
	}
}

func tokenTypeToClass(t chroma.TokenType) string {
	switch {
	case t == chroma.Keyword || t.InSubCategory(chroma.Keyword):
		return "keyword"
	case t == chroma.NameFunction || t == chroma.NameBuiltin:
		return "function"
	case t == chroma.NameClass || t == chroma.NameDecorator:
		return "type"
	case t.InCategory(chroma.Name):
		return "name"
	case t.InCategory(chroma.LiteralString):
		return "string"
	case t.InCategory(chroma.LiteralNumber):
		return "number"
	case t.InCategory(chroma.Comment):
		return "comment"
	case t.InCategory(chroma.Operator):
		return "operator"
	case t == chroma.Punctuation:
		return "punctuation"
	default:
		return "plain"
	}
}

// highlightLine splits a single line's text into highlighted spans using
// precomputed span data for the full code block. lineOffset is the byte
// offset of this line within the full source.
func highlightLine(text string, allSpans []HighlightSpan, lineOffset int) []HighlightSpan {
	if len(allSpans) == 0 {
		return []HighlightSpan{{Text: text, Class: "plain"}}
	}

	// Walk through allSpans finding the subset that covers [lineOffset, lineOffset+len(text))
	var result []HighlightSpan
	pos := 0
	spanPos := 0

	// Advance to the span covering lineOffset
	for _, sp := range allSpans {
		spanEnd := spanPos + len(sp.Text)
		if spanEnd <= lineOffset {
			spanPos = spanEnd
			continue
		}
		if spanPos >= lineOffset+len(text) {
			break
		}

		// Compute the overlap
		overlapStart := spanPos
		if overlapStart < lineOffset {
			overlapStart = lineOffset
		}
		overlapEnd := spanEnd
		if overlapEnd > lineOffset+len(text) {
			overlapEnd = lineOffset + len(text)
		}

		// Fill any gap before this span with plain text
		gapStart := overlapStart - lineOffset
		if gapStart > pos {
			result = append(result, HighlightSpan{
				Text:  text[pos:gapStart],
				Class: "plain",
			})
			pos = gapStart
		}

		localStart := overlapStart - lineOffset
		localEnd := overlapEnd - lineOffset
		if localEnd > pos {
			if localStart < pos {
				localStart = pos
			}
			result = append(result, HighlightSpan{
				Text:  text[localStart:localEnd],
				Class: sp.Class,
			})
			pos = localEnd
		}

		spanPos = spanEnd
	}

	// Trailing text
	if pos < len(text) {
		result = append(result, HighlightSpan{
			Text:  text[pos:],
			Class: "plain",
		})
	}

	return result
}

// splitHighlightByLines takes the full-block spans and splits them per-line
// so each line can be rendered independently.
func splitHighlightByLines(source string, spans []HighlightSpan) [][]HighlightSpan {
	lines := strings.Split(source, "\n")
	result := make([][]HighlightSpan, len(lines))

	offset := 0
	for i, line := range lines {
		result[i] = highlightLine(line, spans, offset)
		offset += len(line) + 1 // +1 for the \n
	}
	return result
}
