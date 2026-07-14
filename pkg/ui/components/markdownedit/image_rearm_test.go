package markdownedit

import (
	"bytes"
	stdimage "image"
	"image/color"
	"image/gif"
	"os"
	"path/filepath"
	"testing"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/terminal"
	"rune/pkg/ui/components/image"
)

// writeAnimatedGIF writes a real 2-frame GIF (16x48 px — tall enough to
// reserve >1 display row at the default 8x16 cell size, which is what makes
// the embed visible to visibleRowsByPath once decoded).
func writeAnimatedGIF(t *testing.T, path string) {
	t.Helper()
	palette := color.Palette{color.White, color.Black}
	g := &gif.GIF{LoopCount: 0} // 0 = loop forever
	for frame := 0; frame < 2; frame++ {
		img := stdimage.NewPaletted(stdimage.Rect(0, 0, 16, 48), palette)
		for x := 0; x < 16; x++ {
			for y := 0; y < 48; y++ {
				img.SetColorIndex(x, y, uint8((frame+x+y)%2))
			}
		}
		g.Image = append(g.Image, img)
		g.Delay = append(g.Delay, 5)
	}
	var buf bytes.Buffer
	if err := gif.EncodeAll(&buf, g); err != nil {
		t.Fatalf("encode gif: %v", err)
	}
	if err := os.WriteFile(path, buf.Bytes(), 0o644); err != nil {
		t.Fatalf("write gif: %v", err)
	}
}

// TestAnimatedImageArmsOnLiveEdge locks in the async-Live re-arm: an animated
// embed driven decode -> SetFrameIDs -> transmit -> Live purely by its own
// lifecycle messages must start ticking (advance past frame 0) WITHOUT any
// subsequent key/scroll/resize input. Regression test for the M4/M5 gap where
// updateImages skipped syncImageViewState, so the Live instance kept its
// pre-decode visibleRows==0 projection and canTick never armed — the GIF sat
// frozen until the next user input.
func TestAnimatedImageArmsOnLiveEdge(t *testing.T) {
	image.DisableFrameDelayForTesting() // ticks resolve synchronously, no real sleeps
	image.DisableTTYWritesForTesting()  // TransmitAnimationCmd runs for real, sans TTY

	tmpDir := t.TempDir()
	writeAnimatedGIF(t, filepath.Join(tmpDir, "anim.gif"))

	m := newImagePipelineModel(t, terminal.TermCaps{GraphicsProtocol: terminal.GraphicsKitty, TrueColor: true})
	// Unfocused: the cursor-on-embed-line reveal rule (§2.3) would otherwise
	// render the embed as source, and StandaloneImagePath skips Revealed
	// spans — mirrors TestDiscoverImagesOnLoad's setup.
	m = m.SetFocused(false)
	m, _ = m.SetContent("![[anim.gif]]") // no docPath yet, so its own discovery no-ops
	m = m.SetDocPath(filepath.Join(tmpDir, "note.md"))

	m, cmd := m.syncImageSet()
	if cmd == nil {
		t.Fatal("setup: expected a decode Cmd for the embed")
	}

	// Pump ONLY the model's own async results back through Update — never a
	// key, wheel, or resize message. Bounded: with frame delays disabled a
	// looping GIF ticks forever, so stop as soon as a frame advance proves
	// the loop armed.
	queue := runImageCmd(cmd)
	var firstID, lastID uint32
	sawLive := false
	for step := 0; step < 32 && len(queue) > 0; step++ {
		msg := queue[0]
		queue = queue[1:]
		var next tea.Cmd
		m, next = m.Update(msg)
		queue = append(queue, runImageCmd(next)...)

		img, ok := m.images["anim.gif"]
		if !ok {
			t.Fatal("tracked instance vanished mid-pump")
		}
		if img.State() == image.Failed {
			t.Fatalf("image failed: %v", img.Err())
		}
		if img.State() == image.Live {
			if !sawLive {
				sawLive = true
				firstID = img.CurrentID()
			}
			lastID = img.CurrentID()
			if lastID != firstID {
				break // frame advanced — the tick loop is armed and running
			}
		}
	}

	if !sawLive {
		t.Fatal("image never reached Live from its own lifecycle messages")
	}
	if lastID == firstID {
		t.Fatal("animated image reached Live but never advanced past frame 0 — the Live edge did not re-arm ticking (visibleRows stale)")
	}
}
