package markdownedit

import (
	"strings"
	"testing"

	"rune/pkg/terminal"
	"rune/pkg/ui/components/textedit"
	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"
)

const faintSGR = "\x1b[2m"

func dimTestModel(t *testing.T, content string, focused bool) Model {
	t.Helper()
	m := New(keymap.Default(), styles.Default(), terminal.TermCaps{})
	m, _ = m.SetRect(textedit.Rect{W: 80, H: 6})
	m, _ = m.SetContent(content)
	m = m.SetFocused(focused)
	return m
}

func firstLine(s string) string {
	line, _, _ := strings.Cut(s, "\n")
	return line
}

// TestView_UnfocusedDimsWholeLinkLine verifies BUG2 at the component level: an
// unfocused line containing a styled link must be dimmed across its whole width,
// not just the first run. The content line is line 0 of the view.
func TestView_UnfocusedDimsWholeLinkLine(t *testing.T) {
	content := "see [click](https://example.com) end"

	unfocused := firstLine(dimTestModel(t, content, false).View())
	if got := strings.Count(unfocused, faintSGR); got < 2 {
		t.Errorf("unfocused link line should dim every run (>=2 faint markers), got %d: %q", got, unfocused)
	}

	focused := firstLine(dimTestModel(t, content, true).View())
	if strings.Contains(focused, faintSGR) {
		t.Errorf("focused link line must not be dimmed: %q", focused)
	}
}

// TestView_SearchMatchesNotDimmed verifies dimming is suppressed when search
// matches are visible (so highlights stay legible), even while unfocused.
func TestView_SearchMatchesNotDimmed(t *testing.T) {
	m := dimTestModel(t, "see [click](https://example.com) end", false)
	m = m.SetSearchQuery("end", true)
	if line0 := firstLine(m.View()); strings.Contains(line0, faintSGR) {
		t.Errorf("unfocused view with active search must not dim: %q", line0)
	}
}

// TestView_Pure verifies View() is a pure function of model state (§8.1 Render
// Purity) for the dimmed, link-bearing path changed by this work.
func TestView_Pure(t *testing.T) {
	m := dimTestModel(t, "see [click](https://example.com) end", false)
	first := m.View()
	for i := range 3 {
		if got := m.View(); got != first {
			t.Fatalf("View() is not pure: call %d differs", i+1)
		}
	}
}
