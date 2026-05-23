package breadcrumb

import (
	"path/filepath"
	"strings"

	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"

	"rune/pkg/ui/styles"
)

type SetPathMsg struct{ Path string }

type Model struct {
	path   string
	dirPath string
	width  int
	styles styles.Styles
}

func New(st styles.Styles) Model {
	return Model{styles: st}
}

func (m Model) SetSize(w, h int) Model      { m.width = w; return m }
func (m Model) SetPath(path string) Model   { m.path = path; return m }
func (m Model) SetDir(dir string) Model     { m.dirPath = dir; return m }
func (m Model) Height() int                 { return 1 }

func (m Model) Init() tea.Cmd { return nil }

func (m Model) Update(msg tea.Msg) (Model, tea.Cmd) {
	if msg, ok := msg.(SetPathMsg); ok {
		m.path = msg.Path
	}
	return m, nil
}

func (m Model) View() string {
	var content string
	if m.path == "" {
		if m.dirPath != "" {
			content = buildCrumb(m.dirPath, m.styles)
		} else {
			content = m.styles.BreadcrumbSep.Render("(no file)")
		}
	} else {
		content = buildCrumb(m.path, m.styles)
	}
	return lipgloss.NewStyle().MaxWidth(m.width).Render(content)
}

func buildCrumb(path string, st styles.Styles) string {
	clean := filepath.Clean(path)
	parts := strings.Split(clean, string(filepath.Separator))

	var b strings.Builder
	sep := st.BreadcrumbSep.Render(" > ")
	for i, p := range parts {
		if i > 0 {
			b.WriteString(sep)
		}
		if i < len(parts)-1 {
			b.WriteString(st.Breadcrumb.Render(p + "/"))
		} else {
			b.WriteString(st.Breadcrumb.Render(p))
		}
	}
	return b.String()
}
