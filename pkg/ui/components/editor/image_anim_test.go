package editor

import (
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// animEditor builds a kitty-capable editor with a live animated image (2 frames)
// on line 1, cursor on line 0 so the image stays Rendered.
func animEditor(t *testing.T, loopCount int) Model {
	t.Helper()
	dir := t.TempDir()
	m := docEditor(t, dir, "intro\n![alt](a.gif)\noutro")
	m.images = m.images.upsert(imageEntry{
		path: "a.gif", absPath: filepath.Join(dir, "a.gif"),
		id: 0xA00000, animated: true,
		frameIDs:   []uint32{0xA00001, 0xA00002},
		frameCount: 2,
		delays:     []time.Duration{50 * time.Millisecond, 50 * time.Millisecond},
		loopCount:  loopCount,
		state:      live,
		cols:       6, rows: 4, pxW: 48, pxH: 32,
	})
	m = m.syncDisplay()
	return m
}

func countPlaceholders(s string) int {
	return strings.Count(s, placeholderRune)
}

func TestImageFrameTick_AdvancesFrameOnlyColorChanges(t *testing.T) {
	m := animEditor(t, 0) // loop forever
	m = m.SetFocused(true)

	// Arm ticks and capture the View at frame 0.
	m, cmd := m.armImageTicks()
	if cmd == nil {
		t.Fatal("expected a tick Cmd for a visible animated image")
	}
	view0 := m.View()
	rows0 := m.snapshot.TotalRows
	id0 := m.imageIDFor("a.gif")

	// Advance to frame 1.
	m, _ = m.Update(imageFrameTickMsg{path: "a.gif", gen: m.images.byPath["a.gif"].tickGen, next: 1})

	if e := m.images.byPath["a.gif"]; e.frameIdx != 1 {
		t.Fatalf("frameIdx=%d, want 1", e.frameIdx)
	}
	id1 := m.imageIDFor("a.gif")
	if id1 == id0 {
		t.Error("expected a different frame image ID after advancing")
	}

	view1 := m.View()
	// Geometry unchanged: same row count and same number of placeholder cells.
	if m.snapshot.TotalRows != rows0 {
		t.Errorf("row count changed across frames: %d -> %d", rows0, m.snapshot.TotalRows)
	}
	if countPlaceholders(view0) != countPlaceholders(view1) {
		t.Errorf("placeholder count changed across frames: %d -> %d", countPlaceholders(view0), countPlaceholders(view1))
	}
	if view0 == view1 {
		t.Error("expected View to differ across frames (frame image ID color)")
	}
}

func TestImageFrameTick_StopsAfterLoopCount(t *testing.T) {
	m := animEditor(t, 1) // play exactly once
	gen := m.images.byPath["a.gif"].tickGen + 1
	m.images = m.images.upsert(func() imageEntry {
		e := m.images.byPath["a.gif"]
		e.ticking = true
		e.tickGen = gen
		return e
	}())

	// Tick to frame 1, then the wrap tick (next == frameCount) completes loop 1.
	m, _ = m.Update(imageFrameTickMsg{path: "a.gif", gen: gen, next: 1})
	m, cmd := m.Update(imageFrameTickMsg{path: "a.gif", gen: gen, next: 2})

	e := m.images.byPath["a.gif"]
	if e.ticking {
		t.Error("expected animation to stop ticking after loopCount reached")
	}
	if cmd != nil {
		t.Error("expected no further tick Cmd after loop budget exhausted")
	}
}

func TestArmImageTicks_OffScreenNotScheduled(t *testing.T) {
	m := animEditor(t, 0)
	// Scroll the viewport far past the image rows.
	m.viewport.TopRow = 100

	_, cmd := m.armImageTicks()
	if cmd != nil {
		t.Error("off-screen animated image must not be ticked")
	}
}

func TestArmImageTicks_StaleTickDropped(t *testing.T) {
	m := animEditor(t, 0)
	e := m.images.byPath["a.gif"]
	e.tickGen = 5
	m.images = m.images.upsert(e)

	// A tick from an older generation must be ignored.
	before := m.images.byPath["a.gif"].frameIdx
	m, _ = m.Update(imageFrameTickMsg{path: "a.gif", gen: 4, next: 1})
	if m.images.byPath["a.gif"].frameIdx != before {
		t.Error("stale-generation tick should not advance the frame")
	}
}
