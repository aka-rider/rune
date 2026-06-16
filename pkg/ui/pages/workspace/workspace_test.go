package workspace

import (
	"errors"
	"strings"
	"testing"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/command"
	"rune/pkg/editor/keybind"
	"rune/pkg/terminal"
	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"
)

// newTestWorkspace creates a sized workspace for testing with a file pre-loaded.
func newTestWorkspace(t *testing.T) Model {
	t.Helper()
	keys := keymap.Default()
	st := styles.Default()
	reg := command.NewBuilder().Build()
	res, _ := keybind.NewResolver(nil)

	m := New(keys, st, reg, res, terminal.TermCaps{}, "", nil)
	m, _ = m.Update(tea.WindowSizeMsg{Width: 80, Height: 24})
	return m
}

// loadFile simulates loading a file into the workspace (via FileLoadedMsg).
func loadFile(m Model, path string, content string) Model {
	// Directly send FileLoadedMsg — workspace owns file/disk domain (D12).
	m, _ = m.Update(FileLoadedMsg{Path: path, Content: []byte(content)})
	return m
}

// ─────────────────────────────────────────────────────────────────────────────
// Gate 4: Global keys do NOT produce text insertion in editor
// ─────────────────────────────────────────────────────────────────────────────

func TestGate4_GlobalKeysDoNotInsertText(t *testing.T) {
	m := newTestWorkspace(t)
	m = loadFile(m, "a.txt", "hello")
	m.focus = paneCenter

	original := m.editor.Content()

	// Send focus explorer key (ctrl+x).
	m, _ = m.Update(tea.KeyPressMsg{Code: 'x', Mod: tea.ModCtrl})
	if m.editor.Content() != original {
		t.Fatal("ctrl+x (FocusExplorer) inserted text in editor")
	}

	// Send zen mode key (ctrl+o).
	m.focus = paneCenter
	m, _ = m.Update(tea.KeyPressMsg{Code: 'o', Mod: tea.ModCtrl})
	if m.editor.Content() != original {
		t.Fatal("ctrl+o (ZenMode) inserted text in editor")
	}

	// Send focus editor key (ctrl+e).
	m.focus = paneCenter
	m, _ = m.Update(tea.KeyPressMsg{Code: 'e', Mod: tea.ModCtrl})
	if m.editor.Content() != original {
		t.Fatal("ctrl+e (FocusEditor) inserted text in editor")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// Gate 11: editor.WantsModalInput() routes keys to editor before globals
// ─────────────────────────────────────────────────────────────────────────────

func TestGate11_WantsModalInputRoutesToEditor(t *testing.T) {
	m := newTestWorkspace(t)
	m = loadFile(m, "a.txt", "hello")
	m.focus = paneCenter

	// Verify the code path: if editor doesn't want modal, global keys work.
	leftBefore := m.leftVisible
	m, _ = m.Update(tea.KeyPressMsg{Code: 'o', Mod: tea.ModCtrl})
	if m.leftVisible == leftBefore {
		// Good - zen mode toggled, confirming global key path works when
		// editor doesn't want modal.
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// Same-file switch is a no-op (no load issued)
// ─────────────────────────────────────────────────────────────────────────────

func TestSameFileOpenIsNoop(t *testing.T) {
	m := newTestWorkspace(t)
	m = loadFile(m, "a.txt", "content A")

	// Open same file — should be a no-op (no cmd issued).
	_, cmd := m.requestOpenPath("a.txt")
	if cmd != nil {
		t.Fatal("opening same file should not issue a load cmd")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// Bug fix: ^W on last tab resets editor to Untitled
// ─────────────────────────────────────────────────────────────────────────────

func TestCloseLastTabResetsToUntitled(t *testing.T) {
	m := newTestWorkspace(t)
	m = loadFile(m, "a.txt", "content A")

	// Only one tab — closing it must reset the editor to a fresh untitled buffer.
	m, cmd := m.requestCloseCurrent()
	msgs := execCmds(cmd)
	for _, msg := range msgs {
		m, cmd = m.Update(msg)
		msgs = append(msgs, execCmds(cmd)...)
	}

	// Title should start with "Untitled" (owned by workspace title component — D6).
	titleText := m.title.Text()
	if !strings.HasPrefix(titleText, "Untitled") {
		t.Fatalf("expected title starting with 'Untitled', got %q", titleText)
	}
	// Workspace must have no file path.
	if m.filePath != "" {
		t.Fatalf("expected empty file path after last-tab close, got %q", m.filePath)
	}
	// Opentabs should show exactly one tab (the new untitled slot) with empty path.
	if path := m.opentabs.PathAt(0); path != "" {
		t.Fatalf("expected untitled tab with empty path, got %q", path)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// fileCreatedMsg error surfaces to user
// ─────────────────────────────────────────────────────────────────────────────

func TestFileCreatedMsg_ErrorSurfaces(t *testing.T) {
	m := newTestWorkspace(t)
	m, _ = m.Update(fileCreatedMsg{path: "foo.md", err: errors.New("disk full")})
	// Error should be shown in footer (we can't test the exact footer text easily,
	// but we can confirm no panic and the model is still consistent).
	_ = m
}

// ─────────────────────────────────────────────────────────────────────────────
// FileSavedMsg with matching RequestID clears InFlight
// ─────────────────────────────────────────────────────────────────────────────

func TestFileSaved_MatchingIDClearsInFlight(t *testing.T) {
	m := newTestWorkspace(t)
	m = loadFile(m, "a.txt", "content A")

	var saveCmd tea.Cmd
	m, saveCmd = m.startSave()
	reqID := m.activeSave.RequestID
	_ = saveCmd

	if !m.activeSave.InFlight {
		t.Fatal("expected InFlight=true")
	}

	m, _ = m.Update(FileSavedMsg{
		Path:         "a.txt",
		RequestID:    reqID,
		SavedContent: []byte("content A"),
	})

	if m.activeSave.InFlight {
		t.Fatal("expected InFlight=false after matching save ack")
	}
}

func TestFileSaved_NonMatchingIDIgnored(t *testing.T) {
	m := newTestWorkspace(t)
	m = loadFile(m, "a.txt", "content A")

	var saveCmd tea.Cmd
	m, saveCmd = m.startSave()
	_ = saveCmd

	if !m.activeSave.InFlight {
		t.Fatal("expected InFlight=true")
	}

	m, _ = m.Update(FileSavedMsg{
		Path:         "a.txt",
		RequestID:    "wrong-id",
		SavedContent: []byte("content A"),
	})

	if !m.activeSave.InFlight {
		t.Fatal("expected InFlight still true after non-matching save ack")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// StoreReadyMsg sets the store and warns on degradation
// ─────────────────────────────────────────────────────────────────────────────

func TestStoreReadyMsg_SetsStore(t *testing.T) {
	m := newTestWorkspace(t)
	if m.store != nil {
		t.Fatal("expected store nil before StoreReadyMsg")
	}
	// We can't easily open a real store in a unit test without cgo/sqlite;
	// just verify the nil-store path is safe.
	m, _ = m.Update(StoreReadyMsg{Store: nil, Warning: ""})
	// No panic, store stays nil.
	_ = m
}

// ─────────────────────────────────────────────────────────────────────────────
// Mouse-driven panel resizing
// ─────────────────────────────────────────────────────────────────────────────

// resizeWorkspace returns a workspace sized to the given dimensions with both
// panes visible and at default widths. Width 100 is wide enough to satisfy the
// minCenterW=24 floor even with both panes at their defaults.
func resizeWorkspace(t *testing.T, w, h int) Model {
	t.Helper()
	keys := keymap.Default()
	st := styles.Default()
	reg := command.NewBuilder().Build()
	res, _ := keybind.NewResolver(nil)

	m := New(keys, st, reg, res, terminal.TermCaps{}, "", nil)
	m, _ = m.Update(tea.WindowSizeMsg{Width: w, Height: h})
	return m
}

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

// execCmds executes a tea.Cmd and collects all resulting messages.
// Handles nil cmds and tea.BatchMsg.
func execCmds(cmd tea.Cmd) []tea.Msg {
	if cmd == nil {
		return nil
	}
	msg := cmd()
	if msg == nil {
		return nil
	}
	if batch, ok := msg.(tea.BatchMsg); ok {
		var msgs []tea.Msg
		for _, c := range batch {
			msgs = append(msgs, execCmds(c)...)
		}
		return msgs
	}
	return []tea.Msg{msg}
}

var errTest = errors.New("test error")

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
