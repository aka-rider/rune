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
	if sp.State != display.Rendered {
		return sp.Text
	}
	switch sp.Kind {
	case display.TokenCodeFence:
		return m.renderCodeFenceSpan(sp)
	case display.TokenImage:
		return renderImageFallback(sp.AltText, m.termCaps)
	case display.TokenHeading:
		return m.renderHeadingSpan(sp)
	case display.TokenInlineCode:
		return m.styles.InlineCode.Render(sp.Text)
	case display.TokenBold:
		return m.styles.MdBold.Render(sp.Text)
	case display.TokenItalic:
		return m.styles.MdItalic.Render(sp.Text)
	case display.TokenStrikethrough:
		return m.styles.MdStrikethrough.Render(sp.Text)
	case display.TokenTaskList:
		return m.renderTaskListSpan(sp)
	case display.TokenTable:
		return m.renderTableSpan(sp)
	default:
		return sp.Text
	}
}

// renderHeadingSpan styles a heading span based on its level.
func (m Model) renderHeadingSpan(sp display.DisplaySpan) string {
	switch sp.HeadingLevel {
	case 1:
		return m.styles.HeadingH1.Render(sp.Text)
	case 6:
		return m.styles.HeadingH6.Render(sp.Text)
	default:
		return m.styles.Heading.Render(sp.Text)
	}
}

// renderTaskListSpan styles a task list checkbox glyph.
func (m Model) renderTaskListSpan(sp display.DisplaySpan) string {
	if strings.Contains(sp.Text, "☑") {
		return m.styles.TaskChecked.Render(sp.Text)
	}
	return m.styles.TaskUnchecked.Render(sp.Text)
}

// renderTableSpan styles a table line based on its role (header/separator/body).
func (m Model) renderTableSpan(sp display.DisplaySpan) string {
	switch sp.TableRole {
	case display.TableRoleHeader:
		return m.styles.TableHeader.Render(sp.Text)
	case display.TableRoleSeparator:
		return m.styles.TableSeparator.Render(sp.Text)
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

type selInterval struct{ start, end int }

// isInSelection reports whether the byte offset falls within any selection interval.
func isInSelection(off int, selections []selInterval) bool {
	for _, sel := range selections {
		if off >= sel.start && off < sel.end {
			return true
		}
	}
	return false
}
