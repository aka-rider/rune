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
	path         string
	dirPath      string
	untitledName string
	width        int
	styles       styles.Styles
}

func New(st styles.Styles, _ func(string) error) Model {
	return Model{styles: st}
}

func (m Model) SetSize(w, _ int) Model             { m.width = w; return m }
func (m Model) SetPath(path string) Model           { m.path = path; return m }
func (m Model) SetDir(dir string) Model             { m.dirPath = dir; return m }
func (m Model) SetUntitledName(name string) Model   { m.untitledName = name; return m }
func (m Model) Height() int                         { return 1 }

func (m Model) Init() tea.Cmd { return nil }

func (m Model) Update(msg tea.Msg) (Model, tea.Cmd) {
	switch msg := msg.(type) {
	case SetPathMsg:
		m.path = msg.Path
	}
	return m, nil
}

func (m Model) View() string {
	cwd, _ := filepath.Abs(".")
	var targetPath string
	if m.path == "" {
		// When path is empty, it's an untitled file.
		// If dirPath isn't set, default it to CWD to ensure it renders relative to vault.
		dir := m.dirPath
		if dir == "" {
			dir = cwd
		}

		name := m.untitledName
		if name == "" {
			name = "Untitled.md" // Safe fallback
		}
		targetPath = filepath.Join(dir, name)
	} else {
		targetPath = m.path
	}

	if targetPath == "" {
		return ""
	}

	return buildCrumb(targetPath, m.styles, cwd, m.width)
}

func buildCrumb(path string, st styles.Styles, cwd string, maxWidth int) string {
	var relPath string
	if cwd != "" {
		absCwd, err := filepath.Abs(cwd)
		if err == nil {
			cwd = absCwd
		}

		absPath, err := filepath.Abs(path)
		if err == nil {
			path = absPath
		}

		// String Prefix Replacement logic
		if strings.HasPrefix(path, cwd) {
			remainder := strings.TrimPrefix(path, cwd)
			// Remove leading slash if present
			remainder = strings.TrimPrefix(remainder, string(filepath.Separator))

			baseName := filepath.Base(cwd)
			if remainder == "" {
				relPath = baseName
			} else {
				relPath = filepath.Join(baseName, remainder)
			}
		} else {
			// Fallback to absolute if it's not under CWD
			relPath = path
		}
	} else {
		relPath = path
	}

	clean := filepath.Clean(relPath)
	parts := strings.Split(clean, string(filepath.Separator))

	sep := st.BreadcrumbSep.Render(" / ")
	ellipsis := st.Breadcrumb.Render("... / ")

	// Start building from right to left to see how much we can fit
	var renderedParts []string
	currentWidth := 0

	for i := len(parts) - 1; i >= 0; i-- {
		partStr := parts[i]
		var renderedPart string
		if i < len(parts)-1 {
			renderedPart = st.Breadcrumb.Render(partStr) + sep
		} else {
			renderedPart = st.Breadcrumb.Render(partStr)
		}

		partWidth := lipgloss.Width(renderedPart)

		// Check if we need to truncate
		// 6 is an arbitrary buffer for the ellipsis and some breathing room
		if currentWidth+partWidth+6 > maxWidth && i > 0 {
			// We can't fit this part, add ellipsis and stop
			renderedParts = append([]string{ellipsis}, renderedParts...)
			break
		}

		renderedParts = append([]string{renderedPart}, renderedParts...)
		currentWidth += partWidth
	}

	return strings.Join(renderedParts, "")
}
