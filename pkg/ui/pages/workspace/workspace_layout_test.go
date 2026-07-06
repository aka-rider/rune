package workspace

import (
	"errors"
	"strings"
	"testing"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/ui/components/filetree"
)

// ─────────────────────────────────────────────────────────────────────────────
// Mouse-driven panel resizing
// ─────────────────────────────────────────────────────────────────────────────

func TestDividerAtPoint(t *testing.T) {
	const W, H = 100, 30
	m := resizeWorkspace(t, W, H)
	m.rightVisible = true
	m = m.recalcLayout()

	contentMidY := 5

	cases := []struct {
		name string
		x    int
		want dragState
	}{
		{"left divider inside col", m.leftPaneW - 1, dragLeft},
		{"left divider outside col", m.leftPaneW, dragLeft},
		{"right divider inside col", W - m.rightPaneW - 1, dragRight},
		{"right divider outside col", W - m.rightPaneW, dragRight},
		{"center mid", W / 2, dragNone},
		{"left pane interior", 5, dragNone},
		{"right pane interior", W - 5, dragNone},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			got, ok := m.dividerAtPoint(c.x, contentMidY)
			if c.want == dragNone {
				if ok {
					t.Fatalf("x=%d: expected no divider, got %v", c.x, got)
				}
				return
			}
			if !ok || got != c.want {
				t.Fatalf("x=%d: expected %v, got %v (ok=%v)", c.x, c.want, got, ok)
			}
		})
	}

	// Outside content height → no divider.
	if _, ok := m.dividerAtPoint(m.leftPaneW-1, H); ok {
		t.Fatal("y past content height should not resolve to a divider")
	}
}

func TestDividerAtPointHiddenPanes(t *testing.T) {
	const W, H = 100, 30
	contentMidY := 5

	t.Run("left hidden: only x=0 restores", func(t *testing.T) {
		m := resizeWorkspace(t, W, H)
		m.leftVisible = false
		m = m.recalcLayout()

		if d, ok := m.dividerAtPoint(0, contentMidY); !ok || d != dragLeft {
			t.Fatalf("x=0 should be left restore, got d=%v ok=%v", d, ok)
		}
		// x=1 is editor content — must NOT trigger restore.
		if d, ok := m.dividerAtPoint(1, contentMidY); ok {
			t.Fatalf("x=1 must be editor content, got divider %v", d)
		}
	})

	t.Run("right hidden: only x=W-1 restores", func(t *testing.T) {
		m := resizeWorkspace(t, W, H)
		m.rightVisible = false
		m = m.recalcLayout()

		if d, ok := m.dividerAtPoint(W-1, contentMidY); !ok || d != dragRight {
			t.Fatalf("x=W-1 should be right restore, got d=%v ok=%v", d, ok)
		}
		// x=W-2 is editor content — must NOT trigger restore.
		if d, ok := m.dividerAtPoint(W-2, contentMidY); ok {
			t.Fatalf("x=W-2 must be editor content, got divider %v", d)
		}
	})

	t.Run("both hidden", func(t *testing.T) {
		m := resizeWorkspace(t, W, H)
		m.leftVisible = false
		m.rightVisible = false
		m = m.recalcLayout()

		if d, ok := m.dividerAtPoint(0, contentMidY); !ok || d != dragLeft {
			t.Fatalf("x=0 should be left restore, got d=%v ok=%v", d, ok)
		}
		if d, ok := m.dividerAtPoint(W-1, contentMidY); !ok || d != dragRight {
			t.Fatalf("x=W-1 should be right restore, got d=%v ok=%v", d, ok)
		}
		if d, ok := m.dividerAtPoint(W/2, contentMidY); ok {
			t.Fatalf("center mid should not be divider, got %v", d)
		}
	})
}

func TestDragLeftResizeAndHide(t *testing.T) {
	const W, H = 100, 30
	m := resizeWorkspace(t, W, H)
	m.rightVisible = true
	m = m.recalcLayout()

	// Click on the left divider to start drag.
	m, _ = m.Update(tea.MouseClickMsg{X: m.leftPaneW - 1, Y: 5, Button: tea.MouseLeft})
	if m.drag != dragLeft {
		t.Fatalf("expected drag=dragLeft after click on divider, got %v", m.drag)
	}

	// Shrink toward the floor: motion to X=20.
	m, _ = m.Update(tea.MouseMotionMsg{X: 20, Y: 5, Button: tea.MouseLeft})
	if !m.leftVisible || m.leftPaneW != 20 {
		t.Fatalf("expected leftPaneW=20 leftVisible=true, got leftPaneW=%d leftVisible=%v",
			m.leftPaneW, m.leftVisible)
	}

	// Cross below min → hide.
	m, _ = m.Update(tea.MouseMotionMsg{X: minLeftPaneW - 1, Y: 5, Button: tea.MouseLeft})
	if m.leftVisible {
		t.Fatalf("expected leftVisible=false after motion below min, got true")
	}
	if m.drag != dragNone {
		t.Fatalf("expected drag cleared after hiding, got %v", m.drag)
	}
	if m.leftPaneW != defaultLeftPaneW {
		t.Fatalf("expected leftPaneW reset to default, got %d", m.leftPaneW)
	}
}

func TestDragLeftFocusFollowsHide(t *testing.T) {
	const W, H = 100, 30
	m := resizeWorkspace(t, W, H)
	m.focus = paneTree

	m, _ = m.Update(tea.MouseClickMsg{X: m.leftPaneW - 1, Y: 5, Button: tea.MouseLeft})
	m, _ = m.Update(tea.MouseMotionMsg{X: 1, Y: 5, Button: tea.MouseLeft})

	if m.leftVisible {
		t.Fatal("expected left hidden")
	}
	if m.focus != paneCenter {
		t.Fatalf("expected focus moved to paneCenter, got %v", m.focus)
	}
}

func TestDragLeftCenterFloor(t *testing.T) {
	const W, H = 100, 30
	m := resizeWorkspace(t, W, H)
	m.rightVisible = true
	m.rightPaneW = defaultRightPaneW
	m = m.recalcLayout()

	m, _ = m.Update(tea.MouseClickMsg{X: m.leftPaneW - 1, Y: 5, Button: tea.MouseLeft})

	// Try to expand far past the center floor.
	m, _ = m.Update(tea.MouseMotionMsg{X: W - 10, Y: 5, Button: tea.MouseLeft})

	maxLeft := W - defaultRightPaneW - minCenterW
	if m.leftPaneW != maxLeft {
		t.Fatalf("expected leftPaneW clamped to %d, got %d", maxLeft, m.leftPaneW)
	}
}

func TestDragRightResizeAndHide(t *testing.T) {
	const W, H = 100, 30
	m := resizeWorkspace(t, W, H)
	m.rightVisible = true
	m = m.recalcLayout()

	rightStart := W - m.rightPaneW
	m, _ = m.Update(tea.MouseClickMsg{X: rightStart, Y: 5, Button: tea.MouseLeft})
	if m.drag != dragRight {
		t.Fatalf("expected drag=dragRight, got %v", m.drag)
	}

	// Shrink: motion further right.
	m, _ = m.Update(tea.MouseMotionMsg{X: W - 25, Y: 5, Button: tea.MouseLeft})
	if !m.rightVisible || m.rightPaneW != 25 {
		t.Fatalf("expected rightPaneW=25 rightVisible=true, got rightPaneW=%d rightVisible=%v",
			m.rightPaneW, m.rightVisible)
	}

	// Cross below min → hide.
	m, _ = m.Update(tea.MouseMotionMsg{X: W - (minRightPaneW - 1), Y: 5, Button: tea.MouseLeft})
	if m.rightVisible {
		t.Fatal("expected rightVisible=false after motion below min")
	}
	if m.drag != dragNone {
		t.Fatalf("expected drag cleared, got %v", m.drag)
	}
	if m.rightPaneW != defaultRightPaneW {
		t.Fatalf("expected rightPaneW reset to default, got %d", m.rightPaneW)
	}
}

func TestDragRightFocusFollowsHide(t *testing.T) {
	const W, H = 100, 30
	m := resizeWorkspace(t, W, H)
	m.rightVisible = true
	m = m.recalcLayout()
	m.focus = paneChat

	rightStart := W - m.rightPaneW
	m, _ = m.Update(tea.MouseClickMsg{X: rightStart, Y: 5, Button: tea.MouseLeft})
	m, _ = m.Update(tea.MouseMotionMsg{X: W - 1, Y: 5, Button: tea.MouseLeft})

	if m.rightVisible {
		t.Fatal("expected right hidden")
	}
	if m.focus != paneCenter {
		t.Fatalf("expected focus moved to paneCenter, got %v", m.focus)
	}
}

func TestDragRightCenterFloor(t *testing.T) {
	const W, H = 100, 30
	m := resizeWorkspace(t, W, H)
	m.rightVisible = true
	m = m.recalcLayout()

	rightStart := W - m.rightPaneW
	m, _ = m.Update(tea.MouseClickMsg{X: rightStart, Y: 5, Button: tea.MouseLeft})
	// Drag the right divider far to the left, exceeding center floor.
	m, _ = m.Update(tea.MouseMotionMsg{X: 10, Y: 5, Button: tea.MouseLeft})

	maxRight := W - defaultLeftPaneW - minCenterW
	if m.rightPaneW != maxRight {
		t.Fatalf("expected rightPaneW clamped to %d, got %d", maxRight, m.rightPaneW)
	}
}

func TestRestoreHiddenLeftOnEdgeClick(t *testing.T) {
	const W, H = 100, 30
	m := resizeWorkspace(t, W, H)
	m.leftVisible = false
	m = m.recalcLayout()

	m, _ = m.Update(tea.MouseClickMsg{X: 0, Y: 5, Button: tea.MouseLeft})

	if !m.leftVisible {
		t.Fatal("expected leftVisible=true after edge click")
	}
	if m.leftPaneW != minLeftPaneW {
		t.Fatalf("expected leftPaneW=minLeftPaneW=%d, got %d", minLeftPaneW, m.leftPaneW)
	}
}

func TestRestoreHiddenRightOnEdgeClick(t *testing.T) {
	const W, H = 100, 30
	m := resizeWorkspace(t, W, H)
	m.rightVisible = false
	m = m.recalcLayout()

	m, _ = m.Update(tea.MouseClickMsg{X: W - 1, Y: 5, Button: tea.MouseLeft})

	if !m.rightVisible {
		t.Fatal("expected rightVisible=true after edge click")
	}
	if m.rightPaneW != minRightPaneW {
		t.Fatalf("expected rightPaneW=minRightPaneW=%d, got %d", minRightPaneW, m.rightPaneW)
	}
}

func TestMotionWithoutDragDoesNotResize(t *testing.T) {
	const W, H = 100, 30
	m := resizeWorkspace(t, W, H)
	m.rightVisible = true
	m = m.recalcLayout()

	beforeLeft := m.leftPaneW
	beforeRight := m.rightPaneW

	m, _ = m.Update(tea.MouseMotionMsg{X: 40, Y: 5, Button: tea.MouseLeft})

	if m.leftPaneW != beforeLeft || m.rightPaneW != beforeRight {
		t.Fatalf("motion without drag changed widths: leftPaneW %d→%d rightPaneW %d→%d",
			beforeLeft, m.leftPaneW, beforeRight, m.rightPaneW)
	}
}

func TestClickClearsStaleDrag(t *testing.T) {
	const W, H = 100, 30
	m := resizeWorkspace(t, W, H)
	m.drag = dragLeft

	// Click in the editor interior (not on a divider).
	m, _ = m.Update(tea.MouseClickMsg{X: W / 2, Y: 5, Button: tea.MouseLeft})

	if m.drag != dragNone {
		t.Fatalf("expected drag cleared by non-divider click, got %v", m.drag)
	}
}

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
