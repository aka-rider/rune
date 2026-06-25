package image

import (
	"path/filepath"
	"testing"

	"time"

	tea "charm.land/bubbletea/v2"
	"rune/pkg/imagekit"
	"rune/pkg/terminal"
)

func runCmd(t *testing.T, cmd tea.Cmd) []tea.Msg {
	if cmd == nil {
		return nil
	}
	msg := cmd()
	if batch, ok := msg.(tea.BatchMsg); ok {
		var out []tea.Msg
		for _, c := range batch {
			out = append(out, runCmd(t, c)...)
		}
		return out
	}
	if msg == nil {
		return nil
	}
	return []tea.Msg{msg}
}

func firstMsg[T tea.Msg](msgs []tea.Msg) (T, bool) {
	for _, m := range msgs {
		if t, ok := m.(T); ok {
			return t, true
		}
	}
	var zero T
	return zero, false
}

func TestImageLifecycle(t *testing.T) {
	dir := t.TempDir()
	path := "test.png"
	absPath := filepath.Join(dir, path)

	caps := terminal.TermCaps{GraphicsProtocol: terminal.GraphicsKitty, TrueColor: true}
	m := New(path, absPath, 1, 0, caps, imagekit.CellSize{}, 80, 24, nil)

	// Step 2: Feed decodedMsg
	m, cmd := m.Update(UpdateMsg{
		Path: path,
		inner: decodedMsg{
			path: path, cols: 6, rows: 4, pxW: 48, pxH: 32,
		},
	})

	if m.Cols() != 6 || m.rows != 4 {
		t.Errorf("image dims incorrectly assigned: %dx%d", m.Cols(), m.rows)
	}

	// Should trigger transmit
	if cmd == nil {
		t.Error("expected TransmitCmd")
	}

	// Step 3: Feed transmittedMsg
	m, cmd = m.Update(UpdateMsg{Path: path, inner: transmittedMsg{path: path}})
	
	if !m.IsLive() {
		t.Errorf("expected image to be live")
	}
}

func TestAnimatedFrames(t *testing.T) {
	caps := terminal.TermCaps{GraphicsProtocol: terminal.GraphicsKitty, TrueColor: true}
	m := New("anim.gif", "anim.gif", 2, 0, caps, imagekit.CellSize{}, 80, 24, nil)
	
	m, _ = m.Update(UpdateMsg{
		Path: "anim.gif",
		inner: decodedMsg{
			path: "anim.gif",
			animated: true,
			frameCount: 2,
			delays: []time.Duration{time.Millisecond*50, time.Millisecond*50},
			loopCount: 1, // loops once
		},
	})
	
	m, _ = m.SetFrameIDs([]uint32{201, 202})
	
	m, _ = m.Update(UpdateMsg{Path: "anim.gif", inner: transmittedMsg{path: "anim.gif"}})
	
	if m.CurrentID() != 201 {
		t.Errorf("expected frame 0 to be active ID, got %d", m.CurrentID())
	}
	
	m = m.SetVisibleRows(10) // will arm ticks
	
	m, cmd := m.ArmTick() 
	if cmd == nil {
		t.Fatalf("expected tick armed")
	}
	
	msgs := runCmd(t, cmd)
	tickMsg, ok := firstMsg[UpdateMsg](msgs)
	if !ok {
		t.Fatalf("expected UpdateMsg tick")
	}
	
	inner, ok := tickMsg.inner.(frameTickMsg)
	if !ok || inner.next != 1 {
		t.Fatalf("expected tick to next frame: %v", tickMsg)
	}
	
	// Apply tick
	m, _ = m.Update(tickMsg)
	if m.CurrentID() != 202 {
		t.Errorf("expected active frame 1 (ID 202), got %d", m.CurrentID())
	}
}
