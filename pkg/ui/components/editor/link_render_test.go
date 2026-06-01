package editor

import (
	"testing"

	"charm.land/lipgloss/v2"

	"rune/pkg/editor/display"
)

// styleKey renders a marker through a style so two lipgloss.Styles can be
// compared for equality by their effective output (lipgloss has no direct ==).
func styleKey(s lipgloss.Style) string { return s.Render("X") }

// firstRenderedSpan returns the first Rendered span of the given kind in the
// editor's current display snapshot.
func firstRenderedSpan(t *testing.T, m Model, kind display.TokenKind) display.DisplaySpan {
	t.Helper()
	for _, l := range m.snapshot.Lines {
		for _, sp := range l.Spans {
			if sp.Kind == kind && sp.State == display.Rendered {
				return sp
			}
		}
	}
	t.Fatalf("no Rendered span of kind %v in snapshot", kind)
	return display.DisplaySpan{}
}

// cellStyleKey renders the span to cells and returns the style key of its first
// non-empty cell.
func cellStyleKey(t *testing.T, m Model, sp display.DisplaySpan) string {
	t.Helper()
	cells := m.spanToCellsStyled(sp)
	if len(cells) == 0 {
		t.Fatalf("span %q produced no cells", sp.Text)
	}
	return styleKey(cells[0].Style)
}

// TestLinkRender_NavigableParity verifies that a markdown link [text](u) and a
// wiki link [[note]] render with the SAME (Link) style — they share LinkRole
// LinkRoleNavigable.
func TestLinkRender_NavigableParity(t *testing.T) {
	// Put constructs on lines after line 0; cursor stays at offset 0 so the
	// spans fold (Rendered) rather than reveal.
	m := newTestEditor("intro\n[text](https://example.com)\n[[note]]\n")
	m = m.syncDisplay()

	link := firstRenderedSpan(t, m, display.TokenLink)
	wiki := firstRenderedSpan(t, m, display.TokenWikiLink)

	if link.LinkRole() != display.LinkRoleNavigable {
		t.Errorf("markdown link LinkRole = %v, want Navigable", link.LinkRole())
	}
	if wiki.LinkRole() != display.LinkRoleNavigable {
		t.Errorf("wiki link LinkRole = %v, want Navigable", wiki.LinkRole())
	}

	want := styleKey(m.styles.Link)
	if got := cellStyleKey(t, m, link); got != want {
		t.Errorf("markdown link cell style != Link style")
	}
	if got := cellStyleKey(t, m, wiki); got != want {
		t.Errorf("wiki link cell style != Link style")
	}
	if cellStyleKey(t, m, link) != cellStyleKey(t, m, wiki) {
		t.Error("markdown link and wiki link should render with identical style")
	}
}

// TestLinkRender_ImageParity verifies that a markdown image ![alt](x.png) and a
// wiki image ![[x.png]] both render with the plain (alt-text fallback) style —
// they share LinkRole LinkRoleImage.
func TestLinkRender_ImageParity(t *testing.T) {
	m := newTestEditor("intro\n![alt](photo.png)\n![[image.png]]\n")
	m = m.syncDisplay()

	mdImg := firstRenderedSpan(t, m, display.TokenImage)
	wikiImg := firstRenderedSpan(t, m, display.TokenWikiLink)

	if mdImg.LinkRole() != display.LinkRoleImage {
		t.Errorf("markdown image LinkRole = %v, want Image", mdImg.LinkRole())
	}
	if wikiImg.LinkRole() != display.LinkRoleImage {
		t.Errorf("wiki image LinkRole = %v, want Image", wikiImg.LinkRole())
	}

	plain := styleKey(lipgloss.NewStyle())
	if got := cellStyleKey(t, m, mdImg); got != plain {
		t.Errorf("markdown image cell style != plain style")
	}
	if got := cellStyleKey(t, m, wikiImg); got != plain {
		t.Errorf("wiki image cell style != plain style")
	}
	if cellStyleKey(t, m, mdImg) != cellStyleKey(t, m, wikiImg) {
		t.Error("markdown image and wiki image should render with identical style")
	}
}

// TestLinkRender_ViewIsPure verifies View() is a pure function: calling it twice
// on the same model yields identical output.
func TestLinkRender_ViewIsPure(t *testing.T) {
	m := newTestEditor("intro\n[text](https://example.com)\n![alt](photo.png)\n[[note]]\n![[image.png]]\n")
	m = m.syncDisplay()

	v1 := m.View()
	v2 := m.View()
	if v1 != v2 {
		t.Error("View() is not pure: two consecutive calls produced different output")
	}
}
