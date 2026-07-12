package workspace

import (
	"errors"
	"strings"
	"testing"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/ui/components/filetree"
)

// ─────────────────────────────────────────────────────────────────────────────
// Footer visibility & error banner (mouse-driven divider-drag suite:
// workspace_divider_drag_test.go)

func TestLayoutFooterAlwaysVisible(t *testing.T) {
	m := newTestWorkspace(t)
	view := m.View()
	lines := strings.Split(view.Content, "\n")
	if len(lines) != 24 {
		t.Fatalf("expected 24 lines, got %d", len(lines))
	}
	lastLine := lines[len(lines)-1]
	if !strings.Contains(lastLine, "Ln") {
		t.Fatalf("footer not on last line: %q", lastLine)
	}
}

func TestLayoutFooterVisibleAfterResize(t *testing.T) {
	m := newTestWorkspace(t)
	for _, size := range []struct{ w, h int }{{40, 10}, {120, 50}, {80, 24}} {
		m, _ = m.Update(tea.WindowSizeMsg{Width: size.w, Height: size.h})
		view := m.View()
		lines := strings.Split(view.Content, "\n")
		if len(lines) != size.h {
			t.Fatalf("at %dx%d: expected %d lines, got %d", size.w, size.h, size.h, len(lines))
		}
	}
}

// TestLayoutErrorBannerViaMessagePathKeepsFooterVisible is the regression
// test for §4: the error banner used to be prepended as an EXTRA row above
// the existing height budget (View: JoinVertical(errLine, body, footer)),
// which MaxHeight(totalHeight) then satisfied by clipping the footer's
// bottom row off — invisible during a resize-then-assert test because
// recalcLayout's topOffset partially compensated, but m.err is set/cleared
// via ErrMsg (workspace_update.go), a path that never calls recalcLayout.
// This test drives m.err via that REAL message path (not a direct field
// assignment after a resize) and asserts the total line count is still
// exactly the window height, with the footer still on the last line.
func TestLayoutErrorBannerViaMessagePathKeepsFooterVisible(t *testing.T) {
	const W, H = 100, 30
	m := resizeWorkspace(t, W, H)

	m, _ = m.Update(ErrMsg{Err: errors.New("disk read failed")})

	view := m.View()
	lines := strings.Split(view.Content, "\n")
	if len(lines) != H {
		t.Fatalf("with error banner set: expected %d lines, got %d", H, len(lines))
	}
	lastLine := lines[len(lines)-1]
	if !strings.Contains(lastLine, "Ln") {
		t.Fatalf("footer not on last line with error banner set: %q", lastLine)
	}
	// The error text itself must still be visible somewhere in the frame.
	if !strings.Contains(view.Content, "disk read failed") {
		t.Fatalf("error text not found in rendered view:\n%s", view.Content)
	}
}

// TestLayoutErrorBannerLongPathOnNarrowWindowKeepsFooterVisible is B1's
// regression test: lipgloss v2's Width(n) WORD-WRAPS content that overflows
// rather than truncating it, so a long path-bearing error rendered on a
// narrow window used to expand overlayErrorLine's top row into several
// physical lines — growing body past recalcLayout's contentH budget, which
// View()'s outer MaxHeight(m.totalHeight) then satisfied by silently
// clipping the footer's bottom row off. Driven through the REAL message path
// (ErrMsg), like its sibling above, with a window narrow enough and an error
// long enough that the pre-fix code was guaranteed to wrap to multiple rows.
func TestLayoutErrorBannerLongPathOnNarrowWindowKeepsFooterVisible(t *testing.T) {
	const W, H = 40, 10
	m := resizeWorkspace(t, W, H)
	// The footer's own content is independent of the top error banner (ErrMsg
	// only sets m.err, never footer.errorMsg) — so its rendered row is the
	// ground truth for "was the footer clipped", regardless of what the
	// default hint bar happens to fit at this width.
	wantFooter := m.footer.View()

	longErr := errors.New("open /very/long/vault/path/that/does/not/fit/in/a/narrow/terminal/window/note.md: permission denied")
	m, _ = m.Update(ErrMsg{Err: longErr})

	view := m.View()
	lines := strings.Split(view.Content, "\n")
	if len(lines) != H {
		t.Fatalf("long error on narrow window: expected %d lines, got %d:\n%s", H, len(lines), view.Content)
	}
	lastLine := lines[len(lines)-1]
	if lastLine != wantFooter {
		t.Fatalf("footer row clipped/altered by a long, word-wrap-prone error banner:\ngot:  %q\nwant: %q", lastLine, wantFooter)
	}
}

// TestSanitizeErrorTextStripsControlBytes is S2's regression test: a raw
// error string carrying C0 control bytes (e.g. an embedded ESC from a
// crafted filename) must never reach the rendered frame verbatim — that
// would inject a terminal escape sequence at the display boundary.
func TestSanitizeErrorTextStripsControlBytes(t *testing.T) {
	in := "bad file \x1b[31mname\x1b[0m.md"
	out := sanitizeErrorText(in)
	if strings.ContainsRune(out, 0x1b) {
		t.Fatalf("sanitizeErrorText left a raw ESC byte in place: %q", out)
	}
	want := "bad file [31mname[0m.md"
	if out != want {
		t.Fatalf("sanitizeErrorText(%q) = %q, want %q", in, out, want)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// Mouse click routing: filetree
// ─────────────────────────────────────────────────────────────────────────────

// TestMouseClickOnUnfocusedFiletreeMovesAndSelects verifies the invariant:
// a left click on the filetree pane while another pane is focused must (a)
// switch focus to paneTree AND (b) move the filetree cursor to the clicked row
// in the same Update — not defer it to the second click.
//
// This is a regression test for the bug where handleMouseClick set m.focus but
// did not forward the click to the filetree, so the first click silently left
// the cursor at its previous position and only the second click moved it.
func TestMouseClickOnUnfocusedFiletreeMovesAndSelects(t *testing.T) {
	const W, H = 100, 30
	m := resizeWorkspace(t, W, H)
	m = m.setFocus(paneCenter) // start with editor focused

	// Populate the filetree with three entries.
	entries := []filetree.Entry{
		{Name: "alpha.md", Path: "/test/alpha.md"},
		{Name: "beta.md", Path: "/test/beta.md"},
		{Name: "gamma.md", Path: "/test/gamma.md"},
	}
	m, _ = m.Update(filetree.DirLoadedMsg{Root: "/test", Entries: entries})
	// DirLoadedMsg resets the cursor to 0.

	// Click on the second entry (row index 1).
	// recalcLayout sets filetree offsetY=1 via SetOffset(1,1), so:
	//   relY = clickY - offsetY = 3 - 1 = 2  →  idx = top + (relY-1) = 0 + 1 = 1
	clickX := m.leftPaneW / 2
	const clickY = 3
	m, _ = m.Update(tea.MouseClickMsg{X: clickX, Y: clickY, Button: tea.MouseLeft})

	if m.focus != paneTree {
		t.Fatalf("after click: focus=%v, want paneTree", m.focus)
	}
	if !m.filetree.Focused() {
		t.Fatal("after click: filetree.Focused()=false, want true")
	}
	// Regression check: old code focused the pane but discarded the click, so
	// the cursor stayed at 0. With the fix, cursor must have moved to 1.
	if got := m.filetree.Cursor(); got != 1 {
		t.Fatalf("first click on row 1: cursor=%d, want 1", got)
	}

	// Second click at the same position: cursor is already at row 1, so the
	// filetree emits FileSelectedMsg (no cursor move).
	m, cmd := m.Update(tea.MouseClickMsg{X: clickX, Y: clickY, Button: tea.MouseLeft})
	if got := m.filetree.Cursor(); got != 1 {
		t.Fatalf("second click: cursor moved to %d, want 1", got)
	}
	var selectedPath string
	for _, msg := range execCmds(cmd) {
		if sel, ok := msg.(filetree.FileSelectedMsg); ok {
			selectedPath = sel.Path
		}
	}
	if selectedPath != "/test/beta.md" {
		t.Fatalf("second click: FileSelectedMsg.Path=%q, want /test/beta.md", selectedPath)
	}
}
