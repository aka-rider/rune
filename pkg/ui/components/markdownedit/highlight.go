package markdownedit

import (
	"github.com/alecthomas/chroma/v2"
	"github.com/alecthomas/chroma/v2/lexers"
)

// HighlightSpan represents a single token with its semantic class.
type HighlightSpan struct {
	Text  string
	Class string // e.g. "keyword", "string", "comment", "plain"
}

// CodeHighlighter tokenizes source code for a given language into styled spans.
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
