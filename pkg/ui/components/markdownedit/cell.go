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
// styling (bold, inline code, headings, tables, code fences with syntax highlight).
func (m Model) spanToCellsStyled(sp display.DisplaySpan) []textedit.Cell {
	if sp.State == display.Revealed {
		return textedit.SpanToCells(sp, lipgloss.NewStyle())
	}

	if sp.Kind == display.TokenTable {
		return m.tableSpanToCells(sp)
	}

	switch sp.LinkRole() {
	case display.LinkRoleImage:
		return textedit.SpanToCells(sp, lipgloss.NewStyle())
	case display.LinkRoleNavigable:
		return textedit.SpanToCells(sp, m.styles.Link)
	}

	switch sp.Kind {
	case display.TokenCodeFence:
		return m.codeFenceSpanToCells(sp)
	case display.TokenHeading:
		return textedit.SpanToCells(sp, m.headingStyle(sp.HeadingLevel))
	case display.TokenInlineCode:
		return textedit.SpanToCells(sp, m.styles.InlineCode)
	case display.TokenBold:
		return textedit.SpanToCells(sp, m.styles.MdBold)
	case display.TokenItalic:
		return textedit.SpanToCells(sp, m.styles.MdItalic)
	case display.TokenStrikethrough:
		return textedit.SpanToCells(sp, m.styles.MdStrikethrough)
	case display.TokenBlockquote:
		return textedit.SpanToCells(sp, m.styles.MdBlockquote)
	case display.TokenHorizontalRule:
		return textedit.SpanToCells(sp, m.styles.HorizontalRule)
	case display.TokenTag:
		return textedit.SpanToCells(sp, m.styles.Tag)
	case display.TokenListMarker:
		return textedit.SpanToCells(sp, m.styles.ListMarker)
	case display.TokenTaskList:
		return m.taskListSpanToCells(sp)
	default:
		return textedit.SpanToCells(sp, lipgloss.NewStyle())
	}
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
	style := mergeInlineStyle(baseStyle, sp.Kind, m.styles)
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
