package breadcrumb

import (
	"path/filepath"
	"strings"
	"unicode/utf8"

	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"

	"rune/pkg/ui/styles"
)

type SetPathMsg struct{ Path string }

// TitleEditCommitMsg is emitted when the user commits a new filename stem.
type TitleEditCommitMsg struct{ Name string }

type Model struct {
	path     string
	dirPath  string
	width    int
	styles   styles.Styles
	editing  bool
	draft    string
	draftErr string
	validate func(string) error
}

func New(st styles.Styles, validate func(string) error) Model {
	return Model{styles: st, validate: validate}
}

func (m Model) SetSize(w, h int) Model    { m.width = w; return m }
func (m Model) SetPath(path string) Model { m.path = path; return m }
func (m Model) SetDir(dir string) Model   { m.dirPath = dir; return m }
func (m Model) Height() int               { return 1 }

// SetEditing enters or exits title edit mode. On entry, seeds draft from the
// current filename stem.
func (m Model) SetEditing(v bool) Model {
	if v && !m.editing {
		stem := strings.TrimSuffix(filepath.Base(m.path), filepath.Ext(m.path))
		m.draft = stem
		m.draftErr = ""
	}
	m.editing = v
	return m
}

func (m Model) Init() tea.Cmd { return nil }

func (m Model) Update(msg tea.Msg) (Model, tea.Cmd) {
	switch msg := msg.(type) {
	case SetPathMsg:
		m.path = msg.Path
		return m, nil

	case tea.KeyPressMsg:
		if !m.editing {
			return m, nil
		}

		// Escape — cancel editing
		if msg.Code == tea.KeyEscape && msg.Mod == 0 {
			m.editing = false
			m.draft = ""
			m.draftErr = ""
			return m, nil
		}

		// Enter — commit if draft is valid and non-empty
		if msg.Code == tea.KeyEnter && msg.Mod == 0 {
			if m.draftErr == "" && m.draft != "" {
				name := m.draft
				m.editing = false
				return m, func() tea.Msg { return TitleEditCommitMsg{Name: name} }
			}
			return m, nil
		}

		// Backspace — remove last rune
		if msg.Code == tea.KeyBackspace && msg.Mod == 0 {
			if len(m.draft) > 0 {
				_, size := utf8.DecodeLastRuneInString(m.draft)
				m.draft = m.draft[:len(m.draft)-size]
			}
			if m.validate != nil {
				if err := m.validate(m.draft); err != nil {
					m.draftErr = err.Error()
				} else {
					m.draftErr = ""
				}
			}
			return m, nil
		}

		// Printable text input
		text := msg.Text
		if text == "" && msg.Mod == 0 && msg.Code >= 32 && msg.Code < 127 {
			text = string(rune(msg.Code))
		}
		if text != "" {
			proposed := m.draft + text
			if m.validate != nil {
				if err := m.validate(proposed); err != nil {
					m.draftErr = err.Error()
					return m, nil
				}
			}
			m.draft = proposed
			m.draftErr = ""
		}
		return m, nil
	}
	return m, nil
}

func (m Model) View() string {
	var content string
	if m.editing {
		content = buildEditingCrumb(m.path, m.draft, m.draftErr, m.styles)
	} else if m.path == "" {
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

func buildEditingCrumb(path, draft, draftErr string, st styles.Styles) string {
	var b strings.Builder

	// Show directory prefix if there is one
	dir := filepath.Dir(path)
	if dir != "." && dir != "" {
		parts := strings.Split(filepath.Clean(dir), string(filepath.Separator))
		sep := st.BreadcrumbSep.Render(" > ")
		for _, p := range parts {
			b.WriteString(st.Breadcrumb.Render(p + "/"))
			b.WriteString(sep)
		}
	}

	// Show draft with cursor
	b.WriteString(st.Breadcrumb.Render("[" + draft + "]▌"))

	if draftErr != "" {
		b.WriteString(" ")
		b.WriteString(st.BreadcrumbSep.Render("✗ " + draftErr))
	}

	return b.String()
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
