package workspace

// paneGeometry's resize-starvation guard (workspace_view.go): a terminal
// resize must collapse side panes before it starves the center below
// minCenterW — the mouse drag path always enforced the minimums, but
// WindowSizeMsg used to bypass them, allocating the editor as little as ONE
// inner column, where a wrap-forced width-2 rune renders no cell for the
// cursor to land on (fuzz invariant R1, corpus pin
// testdata/fuzz/FuzzSession/45de48a20890bdb3).

import (
	"testing"

	tea "charm.land/bubbletea/v2"
)

// TestResize_NarrowTerminal_CursorOnWideRune is the deterministic companion
// to the corpus pin: cursor ON a CJK rune in a 25-column terminal. settle's
// invariant sweep asserts R1 (cursor cell rendered) and L1/L2 (frame within
// terminal bounds — the failure mode of the reverted textedit width-floor,
// which rendered 2 columns into a 1-column box and clipped the footer).
func TestResize_NarrowTerminal_CursorOnWideRune(t *testing.T) {
	m := withStore(t, newScrollWorkspace(t))
	m = loadFile(m, "/docs/wide.md", "ab你cd")
	m = focusEditor(m)

	var cmd tea.Cmd
	m, cmd = m.Update(tea.WindowSizeMsg{Width: 25, Height: 25})
	m = settle(t, m, cmd)

	g := m.paneGeometry()
	if g.CenterW < minCenterW {
		t.Fatalf("center pane starved on resize: CenterW=%d, want >= %d (side panes must collapse first)", g.CenterW, minCenterW)
	}

	// Walk the cursor onto 你 (byte offset 2) through real keys; settle runs
	// the full invariant sweep (R1 included) after every step.
	m, cmd = m.Update(tea.KeyPressMsg{Code: tea.KeyHome})
	m = settle(t, m, cmd)
	m, cmd = m.Update(tea.KeyPressMsg{Code: tea.KeyRight})
	m = settle(t, m, cmd)
	m, cmd = m.Update(tea.KeyPressMsg{Code: tea.KeyRight})
	m = settle(t, m, cmd)

	snap := m.FuzzInspect()
	if len(snap.CursorOffsets) != 1 {
		t.Fatalf("setup: want 1 cursor, got %d", len(snap.CursorOffsets))
	}
	cursorCells := 0
	for _, row := range snap.Cells {
		for _, c := range row {
			if c.Cursor {
				cursorCells++
			}
		}
	}
	if cursorCells != 1 {
		t.Fatalf("R1: want exactly 1 rendered cursor cell, got %d (cursors %v in %q)",
			cursorCells, snap.CursorOffsets, snap.Content)
	}
}

// TestPaneGeometry_NeverStarvesCenter sweeps every terminal width and pane
// combination: geometry always tiles the full width, and whenever the
// terminal can afford minCenterW the center gets at least it.
func TestPaneGeometry_NeverStarvesCenter(t *testing.T) {
	base := newTestWorkspace(t)
	for _, rightVisible := range []bool{false, true} {
		m := base
		// rightVisible is workspace-internal render state with no
		// store-independent key path in this fixture; the property under
		// test is paneGeometry's pure math over its inputs.
		m.rightVisible = rightVisible
		for w := 1; w <= 200; w++ {
			var cmd tea.Cmd
			m, cmd = m.Update(tea.WindowSizeMsg{Width: w, Height: 24})
			m = settle(t, m, cmd)
			g := m.paneGeometry()
			if g.LeftW+g.CenterW+g.RightW != w {
				t.Fatalf("w=%d right=%v: panes do not tile the width: %d+%d+%d",
					w, rightVisible, g.LeftW, g.CenterW, g.RightW)
			}
			if w >= minCenterW && g.CenterW < minCenterW {
				t.Fatalf("w=%d right=%v: center starved: CenterW=%d < %d",
					w, rightVisible, g.CenterW, minCenterW)
			}
		}
	}
}
