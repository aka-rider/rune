package editor

import (
	"strings"

	"charm.land/lipgloss/v2"

	"rune/pkg/editor/display"
	"rune/pkg/ui/styles"
)

// renderSpan renders a single display span, applying code-fence highlighting
// when appropriate. For code fence spans in Rendered state, it applies syntax
// highlighting via the editor's highlighter adapter.
func (m Model) renderSpan(sp display.DisplaySpan) string {
	switch {
	case sp.Kind == display.TokenCodeFence && sp.State == display.Rendered:
		return m.renderCodeFenceSpan(sp)
	default:
		return sp.Text
	}
}

// renderCodeFenceSpan applies syntax highlighting and code-block background
// to a single rendered code fence span.
func (m Model) renderCodeFenceSpan(sp display.DisplaySpan) string {
	// Fence marker lines (opening/closing) have empty text in rendered mode.
	// Use the opening marker to show a language label if declared.
	if sp.Text == "" {
		if sp.Language != "" {
			return renderCodeLabel(sp.Language, m.styles)
		}
		return ""
	}

	// No highlighter or no language — render with code-block background only
	if m.highlighter == nil || sp.Language == "" {
		return m.styles.CodePlain.Render(sp.Text)
	}

	spans, err := m.highlighter(sp.Language, sp.Text)
	if err != nil || spans == nil {
		return m.styles.CodePlain.Render(sp.Text)
	}

	return renderHighlightedSpans(spans, m.styles)
}

// renderHighlightedSpans converts highlight spans to styled terminal output.
func renderHighlightedSpans(spans []HighlightSpan, st styles.Styles) string {
	var b strings.Builder
	for _, s := range spans {
		style := classToStyle(s.Class, st)
		b.WriteString(style.Render(s.Text))
	}
	return b.String()
}

func classToStyle(class string, st styles.Styles) lipgloss.Style {
	switch class {
	case "keyword":
		return st.CodeKeyword
	case "string":
		return st.CodeString
	case "comment":
		return st.CodeComment
	case "function":
		return st.CodeFunction
	case "type":
		return st.CodeType
	case "number":
		return st.CodeNumber
	case "operator":
		return st.CodeOperator
	default:
		return st.CodePlain
	}
}

// renderCodeLabel returns the styled language label for a code block.
func renderCodeLabel(language string, st styles.Styles) string {
	if language == "" {
		return ""
	}
	return st.CodeBlockLabel.Render(language)
}
