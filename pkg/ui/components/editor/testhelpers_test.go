package editor

import (
	"image"
	"image/color"
	"image/png"
	"os"
	"path/filepath"
	"testing"

	"rune/pkg/editor/cursor"
	"rune/pkg/terminal"

	tea "charm.land/bubbletea/v2"
)

// setCursor places a single cursor at the given byte offset.
func setCursor(m Model, offset int) Model {
	m.cursors = cursor.NewCursorSet(offset)
	return m
}

// runCmd executes a tea.Cmd, flattening tea.BatchMsg into a list of messages.
func runCmd(t *testing.T, cmd tea.Cmd) []tea.Msg {
	t.Helper()
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

// writePNG writes a w x h solid PNG to path (creating parent dirs).
func writePNG(t *testing.T, path string, w, h int) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	img := image.NewRGBA(image.Rect(0, 0, w, h))
	for y := 0; y < h; y++ {
		for x := 0; x < w; x++ {
			img.Set(x, y, color.RGBA{R: 30, G: 90, B: 200, A: 255})
		}
	}
	f, err := os.Create(path)
	if err != nil {
		t.Fatal(err)
	}
	defer f.Close()
	if err := png.Encode(f, img); err != nil {
		t.Fatal(err)
	}
}

// docEditor builds a kitty-capable editor whose open file lives in dir, with the
// given content, cursor on line 0.
func docEditor(t *testing.T, dir, content string) Model {
	t.Helper()
	m := newTestEditor("")
	m.termCaps = kittyCaps()
	m = m.SetContent(filepath.Join(dir, "note.md"), []byte(content))
	m = m.SetSize(80, 24)
	m = m.SetFocused(true)
	return m
}

func kittyCaps() terminal.TermCaps {
	return terminal.TermCaps{GraphicsProtocol: terminal.GraphicsKitty, TrueColor: true}
}
