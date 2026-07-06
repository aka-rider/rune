package breadcrumb

import (
	"path/filepath"
	"strings"

	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"

	"rune/pkg/ui/styles"
)

type Model struct {
	path    string
	dirPath string
	width   int
	styles  styles.Styles
}

func New(st styles.Styles) Model {
	return Model{styles: st}
}

func (m Model) SetSize(w, _ int) Model    { m.width = w; return m }
func (m Model) SetPath(path string) Model { m.path = path; return m }
func (m Model) SetDir(dir string) Model   { m.dirPath = dir; return m }
func (m Model) Height() int               { return 1 }

func (m Model) Init() tea.Cmd { return nil }

// View renders the breadcrumb. Pure string math (§1.4.9, §5.2) — no
// filesystem access. m.dirPath and m.path are both injected as absolute
// paths by the caller (the workspace's launch-captured workDir via SetDir,
// and file-load/rename paths via SetPath), so no per-render os.Getwd/
// filepath.Abs is needed to relativize them.
func (m Model) View() string {
	var targetPath string
	if m.path == "" {
		// When path is empty, it's an untitled file — anchor it under the
		// injected workspace dir (empty dirPath renders just the filename).
		targetPath = filepath.Join(m.dirPath, "Untitled.md")
	} else {
		targetPath = m.path
	}

	if targetPath == "" {
		return ""
	}

	return buildCrumb(targetPath, m.styles, m.dirPath, m.width)
}

// buildCrumb relativizes path against root by string prefix — both are
// assumed already-absolute (View's contract above), so this never touches
// the filesystem.
func buildCrumb(path string, st styles.Styles, root string, maxWidth int) string {
	var relPath string
	if root != "" {
		// String Prefix Replacement logic. B3: a bare strings.HasPrefix has no
		// separator boundary, so root=/a/vault would wrongly claim
		// /a/vault2/notes.md too (remainder "2/notes.md" doesn't start with a
		// separator) — accept the match only when the remainder is empty
		// (path == root exactly) or itself starts with a path separator.
		remainder := strings.TrimPrefix(path, root)
		isUnderRoot := strings.HasPrefix(path, root) &&
			(remainder == "" || strings.HasPrefix(remainder, string(filepath.Separator)))
		if isUnderRoot {
			// Remove leading slash if present
			remainder = strings.TrimPrefix(remainder, string(filepath.Separator))

			baseName := filepath.Base(root)
			if remainder == "" {
				relPath = baseName
			} else {
				relPath = filepath.Join(baseName, remainder)
			}
		} else {
			// Fallback to absolute if it's not under root
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
