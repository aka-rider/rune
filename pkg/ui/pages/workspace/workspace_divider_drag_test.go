package workspace

import (
	"testing"

	tea "charm.land/bubbletea/v2"
)

// ─────────────────────────────────────────────────────────────────────────────
// Mouse-driven panel resizing
// ─────────────────────────────────────────────────────────────────────────────

// TestDividerDrag_EveryEdgeColumn replaces the old dividerAtPoint geometry
// table with the REAL input path: a click on either column of a divider must
// start a drag whose subsequent motion actually resizes the matching pane —
// asserted on the resulting pane widths, not on the private hit-test's
// return values.
func TestDividerDrag_EveryEdgeColumn(t *testing.T) {
	const W, H = 100, 30
	cases := []struct {
		name  string
		x     func(m Model) int
		wantL bool // the LEFT pane must resize; otherwise the RIGHT pane must
	}{
		{"left divider inside col", func(m Model) int { return m.leftPaneW - 1 }, true},
		{"left divider outside col", func(m Model) int { return m.leftPaneW }, true},
		{"right divider inside col", func(m Model) int { return W - m.rightPaneW - 1 }, false},
		{"right divider outside col", func(m Model) int { return W - m.rightPaneW }, false},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			m := resizeWorkspace(t, W, H)
			m.rightVisible = true
			m, _ = m.recalcLayout()

			m, _ = m.Update(tea.MouseClickMsg{X: c.x(m), Y: 5, Button: tea.MouseLeft})
			if c.wantL {
				m, _ = m.Update(tea.MouseMotionMsg{X: 20, Y: 5, Button: tea.MouseLeft})
				if m.leftPaneW != 20 {
					t.Fatalf("drag from the left divider column: leftPaneW=%d, want 20", m.leftPaneW)
				}
			} else {
				m, _ = m.Update(tea.MouseMotionMsg{X: W - 25, Y: 5, Button: tea.MouseLeft})
				if m.rightPaneW != 25 {
					t.Fatalf("drag from the right divider column: rightPaneW=%d, want 25", m.rightPaneW)
				}
			}
		})
	}
}

// TestDividerDrag_InteriorClicksNeverDrag: a click anywhere that is NOT a
// divider column (pane interiors, center) followed by motion must leave both
// pane widths untouched — the motion is ordinary mouse movement, not a
// resize.
func TestDividerDrag_InteriorClicksNeverDrag(t *testing.T) {
	const W, H = 100, 30
	for _, c := range []struct {
		name string
		x    func(Model) int // computed from the live model, never a name-keyed special case
		y    int
	}{
		{"center mid", func(Model) int { return W / 2 }, 5},
		{"left pane interior", func(Model) int { return 5 }, 5},
		{"right pane interior", func(Model) int { return W - 5 }, 5},
		{"left divider column but below content height", func(m Model) int { return m.leftPaneW - 1 }, H - 1}, // footer row
	} {
		t.Run(c.name, func(t *testing.T) {
			m := resizeWorkspace(t, W, H)
			m.rightVisible = true
			m, _ = m.recalcLayout()
			beforeLeft, beforeRight := m.leftPaneW, m.rightPaneW
			x := c.x(m)

			m, _ = m.Update(tea.MouseClickMsg{X: x, Y: c.y, Button: tea.MouseLeft})
			m, _ = m.Update(tea.MouseMotionMsg{X: 20, Y: 5, Button: tea.MouseLeft})
			m, _ = m.Update(tea.MouseMotionMsg{X: W - 25, Y: 5, Button: tea.MouseLeft})

			if m.leftPaneW != beforeLeft || m.rightPaneW != beforeRight {
				t.Fatalf("non-divider click at (%d,%d) started a drag: leftPaneW %d→%d rightPaneW %d→%d",
					x, c.y, beforeLeft, m.leftPaneW, beforeRight, m.rightPaneW)
			}
		})
	}
}

// TestHiddenPaneEdge_OnlyOutermostColumnRestores: with a pane hidden, only
// the outermost screen column brings it back — one column in is editor
// content and a click there must NOT restore. (Real clicks; complements
// TestRestoreHiddenLeftOnEdgeClick / TestRestoreHiddenRightOnEdgeClick,
// which pin the positive cases.)
func TestHiddenPaneEdge_OnlyOutermostColumnRestores(t *testing.T) {
	const W, H = 100, 30

	t.Run("left hidden: x=1 is editor content", func(t *testing.T) {
		m := resizeWorkspace(t, W, H)
		m.leftVisible = false
		m, _ = m.recalcLayout()

		m, _ = m.Update(tea.MouseClickMsg{X: 1, Y: 5, Button: tea.MouseLeft})
		if m.leftVisible {
			t.Fatal("click one column in from the left edge must not restore the hidden left pane")
		}
	})

	t.Run("right hidden: x=W-2 is editor content", func(t *testing.T) {
		m := resizeWorkspace(t, W, H)
		m.rightVisible = false
		m, _ = m.recalcLayout()

		m, _ = m.Update(tea.MouseClickMsg{X: W - 2, Y: 5, Button: tea.MouseLeft})
		if m.rightVisible {
			t.Fatal("click one column in from the right edge must not restore the hidden right pane")
		}
	})

	t.Run("both hidden: both edges restore, center does nothing", func(t *testing.T) {
		m := resizeWorkspace(t, W, H)
		m.leftVisible = false
		m.rightVisible = false
		m, _ = m.recalcLayout()

		m, _ = m.Update(tea.MouseClickMsg{X: W / 2, Y: 5, Button: tea.MouseLeft})
		if m.leftVisible || m.rightVisible {
			t.Fatal("center click must not restore either hidden pane")
		}
		m, _ = m.Update(tea.MouseClickMsg{X: 0, Y: 5, Button: tea.MouseLeft})
		if !m.leftVisible {
			t.Fatal("left edge click must restore the hidden left pane")
		}
		m, _ = m.Update(tea.MouseClickMsg{X: W - 1, Y: 5, Button: tea.MouseLeft})
		if !m.rightVisible {
			t.Fatal("right edge click must restore the hidden right pane")
		}
	})
}

func TestDragLeftResizeAndHide(t *testing.T) {
	const W, H = 100, 30
	m := resizeWorkspace(t, W, H)
	m.rightVisible = true
	m, _ = m.recalcLayout()

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
	m, _ = m.recalcLayout()

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
	m, _ = m.recalcLayout()

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
	m, _ = m.recalcLayout()
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
	m, _ = m.recalcLayout()

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
	m, _ = m.recalcLayout()

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
	m, _ = m.recalcLayout()

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
	m, _ = m.recalcLayout()

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
