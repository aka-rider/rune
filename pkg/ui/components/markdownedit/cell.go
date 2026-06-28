package markdownedit

import (
	"strings"
	"unicode/utf8"

	"charm.land/lipgloss/v2"
	"github.com/mattn/go-runewidth"

	"rune/pkg/editor/display"
	"rune/pkg/ui/components/textedit"
	"rune/pkg/ui/styles"
)

// spanToCellsStyled converts a span to styled cells, applying markdown-specific
// styling: the content Kind/LinkRole selects a base style, then composable
// decorations (bold/italic/strike via Marks) are folded on top — so a bold link
// (**[x](y)**) renders as a single underlined+bold span.
func (m Model) spanToCellsStyled(sp display.DisplaySpan) []textedit.Cell {
	if sp.State == display.Revealed {
		return textedit.SpanToCells(sp, lipgloss.NewStyle())
	}

	if sp.Kind == display.TokenTable {
		return m.tableSpanToCells(sp)
	}

	// Tokens with bespoke cell builders.
	switch sp.Kind {
	case display.TokenCodeFence:
		return m.codeFenceSpanToCells(sp)
	case display.TokenTaskList:
		return m.taskListSpanToCells(sp)
	}

	return textedit.SpanToCells(sp, applyMarks(m.inlineBaseStyle(sp), sp.Marks))
}

// inlineBaseStyle returns the content-role base style for a rendered span, before
// decoration marks are applied.
func (m Model) inlineBaseStyle(sp display.DisplaySpan) lipgloss.Style {
	switch sp.LinkRole() {
	case display.LinkRoleImage:
		return lipgloss.NewStyle()
	case display.LinkRoleNavigable:
		return m.styles.Link
	}

	switch sp.Kind {
	case display.TokenHeading:
		return m.headingStyle(sp.HeadingLevel)
	case display.TokenInlineCode:
		return m.styles.InlineCode
	case display.TokenBold:
		return m.styles.MdBold
	case display.TokenItalic:
		return m.styles.MdItalic
	case display.TokenStrikethrough:
		return m.styles.MdStrikethrough
	case display.TokenBlockquote:
		return m.styles.MdBlockquote
	case display.TokenHorizontalRule:
		return m.styles.HorizontalRule
	case display.TokenTag:
		return m.styles.Tag
	case display.TokenListMarker:
		return m.styles.ListMarker
	default:
		return lipgloss.NewStyle()
	}
}

// applyMarks folds composable decorations (bold/italic/strike) onto a base style.
func applyMarks(base lipgloss.Style, m display.InlineMarks) lipgloss.Style {
	if m.Has(display.MarkBold) {
		base = base.Bold(true)
	}
	if m.Has(display.MarkItalic) {
		base = base.Italic(true)
	}
	if m.Has(display.MarkStrikethrough) {
		base = base.Strikethrough(true)
	}
	return base
}

func (m Model) tableRoleStyle(role display.TableRoleKind) lipgloss.Style {
	switch role {
	case display.TableRoleHeader:
		return m.styles.TableHeader
	case display.TableRoleSeparator:
		return m.styles.TableSeparator
	default:
		return m.styles.TableBody
	}
}

func (m Model) headingStyle(level int) lipgloss.Style {
	switch level {
	case 1:
		return m.styles.HeadingH1
	case 2:
		return m.styles.HeadingH2
	case 3:
		return m.styles.HeadingH3
	case 4:
		return m.styles.HeadingH4
	case 5:
		return m.styles.HeadingH5
	case 6:
		return m.styles.HeadingH6
	default:
		return m.styles.HeadingH6
	}
}

func (m Model) taskListSpanToCells(sp display.DisplaySpan) []textedit.Cell {
	style := m.styles.TaskUnchecked
	if strings.Contains(sp.Text, "☑") {
		style = m.styles.TaskChecked
	}
	return textedit.SpanToCells(sp, style)
}

func (m Model) tableSpanToCells(sp display.DisplaySpan) []textedit.Cell {
	baseStyle := m.tableRoleStyle(sp.TableRole)
	style := applyMarks(mergeInlineStyle(baseStyle, sp.Kind, m.styles), sp.Marks)
	cells := textedit.SpanToCells(sp, style)
	for i := range cells {
		if cells[i].Rune == '│' {
			cells[i].Style = m.styles.TableBorder
		}
	}
	return cells
}

func mergeInlineStyle(base lipgloss.Style, kind display.TokenKind, st styles.Styles) lipgloss.Style {
	switch kind {
	case display.TokenBold:
		return base.Bold(true)
	case display.TokenItalic:
		return base.Italic(true)
	case display.TokenStrikethrough:
		return base.Strikethrough(true)
	case display.TokenInlineCode:
		return st.InlineCode.Background(base.GetBackground())
	case display.TokenLink:
		return st.Link.Background(base.GetBackground())
	default:
		return base
	}
}

func (m Model) codeFenceSpanToCells(sp display.DisplaySpan) []textedit.Cell {
	if sp.Text == "" {
		if sp.Language != "" {
			label := sp.Language
			cells := make([]textedit.Cell, 0, len(label))
			pos := 0
			for pos < len(label) {
				r, size := utf8.DecodeRuneInString(label[pos:])
				if size == 0 {
					size = 1
				}
				cells = append(cells, textedit.Cell{
					Rune:      r,
					Width:     runewidth.RuneWidth(r),
					Style:     m.styles.CodeBlockLabel,
					BufOffset: -1,
				})
				pos += size
			}
			return cells
		}
		return nil
	}

	if m.highlighter == nil || sp.Language == "" {
		return textedit.SpanToCells(sp, m.styles.CodePlain)
	}

	spans, err := m.highlighter(sp.Language, sp.Text)
	if err != nil || spans == nil {
		return textedit.SpanToCells(sp, m.styles.CodePlain)
	}

	cells := make([]textedit.Cell, 0, len(sp.Text))
	bufPos := sp.BufferStart
	for _, hs := range spans {
		style := classToStyle(hs.Class, m.styles)
		pos := 0
		for pos < len(hs.Text) {
			r, size := utf8.DecodeRuneInString(hs.Text[pos:])
			if size == 0 {
				size = 1
				r = utf8.RuneError
			}
			w := runewidth.RuneWidth(r)
			if w == 0 {
				w = 1
			}
			cells = append(cells, textedit.Cell{
				Rune:      r,
				Width:     w,
				Style:     style,
				BufOffset: bufPos,
			})
			pos += size
			bufPos += size
		}
	}
	return cells
}
