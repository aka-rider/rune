package imagekit

import (
	"image/color"
	"strings"
	"testing"
)

func TestFitBox(t *testing.T) {
	cases := []struct {
		name           string
		sw, sh, mw, mh int
		wantW, wantH   int
	}{
		{"fits", 10, 10, 100, 100, 10, 10},
		{"width-bound", 200, 100, 100, 100, 100, 50},
		{"height-bound", 100, 200, 100, 100, 50, 100},
		{"square-into-square", 400, 400, 80, 80, 80, 80},
		{"degenerate", 0, 10, 100, 100, 0, 0},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			w, h := FitBox(tc.sw, tc.sh, tc.mw, tc.mh)
			if w != tc.wantW || h != tc.wantH {
				t.Errorf("FitBox=%dx%d, want %dx%d", w, h, tc.wantW, tc.wantH)
			}
		})
	}
}

func TestFitCells(t *testing.T) {
	cs := DefaultCellSize() // 8x16
	// 80x160 px => 10 cols x 10 rows naturally.
	cols, rows := FitCells(80, 160, 100, 100, cs)
	if cols != 10 || rows != 10 {
		t.Errorf("FitCells natural = %dx%d, want 10x10", cols, rows)
	}
	// Constrained by maxCols.
	cols, rows = FitCells(800, 1600, 20, 100, cs)
	if cols > 20 || rows > 100 || cols < 1 || rows < 1 {
		t.Errorf("FitCells constrained = %dx%d, out of bounds", cols, rows)
	}
	if cols != 20 {
		t.Errorf("expected cols clamped to 20, got %d", cols)
	}
	// Degenerate.
	if c, r := FitCells(0, 0, 10, 10, cs); c != 0 || r != 0 {
		t.Errorf("degenerate FitCells = %dx%d, want 0x0", c, r)
	}
}

func TestResize(t *testing.T) {
	src := solidImage(10, 10, color.RGBA{R: 255, A: 255})
	dst := Resize(src, 4, 6)
	if dst.Bounds().Dx() != 4 || dst.Bounds().Dy() != 6 {
		t.Errorf("Resize bounds = %v, want 4x6", dst.Bounds())
	}
	// Non-positive dimensions must not panic.
	if got := Resize(src, 0, 5); got.Bounds().Empty() {
		t.Error("Resize with zero width returned empty bounds")
	}
}

func TestAllocID(t *testing.T) {
	a := AllocID("/abs/path/to/image.png")
	b := AllocID("/abs/path/to/image.png")
	if a != b {
		t.Errorf("AllocID not deterministic: %d != %d", a, b)
	}
	if a == 0 {
		t.Error("AllocID returned zero")
	}
	if a > idMask24 {
		t.Errorf("AllocID %d exceeds 24-bit mask", a)
	}
	if AllocID("/other.png") == a {
		t.Error("AllocID collided for distinct paths (unexpected for these inputs)")
	}
}

func TestEncodeTransmit(t *testing.T) {
	img := solidImage(8, 16, color.RGBA{R: 1, G: 2, B: 3, A: 255})
	id := AllocID("/x.png")
	seq, err := EncodeTransmit(img, id, 1, 1)
	if err != nil {
		t.Fatalf("EncodeTransmit: %v", err)
	}
	// APC introducer is ESC _ G ... ESC \
	if !strings.Contains(seq, "\x1b_G") {
		t.Errorf("output missing APC introducer: %q", snippet(seq))
	}
	// Image ID encoded as i=<id>.
	if !strings.Contains(seq, "i=") {
		t.Error("output missing image ID option")
	}
	// Virtual placement flag.
	if !strings.Contains(seq, "U=1") {
		t.Error("output missing virtual placement (U=1)")
	}
}

func snippet(s string) string {
	if len(s) > 40 {
		return s[:40]
	}
	return s
}

func TestEncodeDelete(t *testing.T) {
	if s := EncodeDelete(42); !strings.Contains(s, "\x1b_G") || !strings.Contains(s, "a=d") {
		t.Errorf("EncodeDelete malformed: %q", s)
	}
	if s := EncodeDeleteAll(); !strings.Contains(s, "\x1b_G") || !strings.Contains(s, "a=d") {
		t.Errorf("EncodeDeleteAll malformed: %q", s)
	}
}

func TestEncodeITerm2(t *testing.T) {
	img := solidImage(8, 16, color.RGBA{R: 1, G: 2, B: 3, A: 255})
	seq, err := EncodeITerm2(img, 4, 2)
	if err != nil {
		t.Fatalf("EncodeITerm2: %v", err)
	}
	// OSC 1337 introducer.
	if !strings.HasPrefix(seq, "\033]1337;") {
		t.Errorf("output missing OSC 1337 prefix: %q", snippet(seq))
	}
	// BEL terminator.
	if !strings.HasSuffix(seq, "\a") {
		t.Errorf("output missing BEL terminator")
	}
	// Required params.
	if !strings.Contains(seq, "inline=1") {
		t.Error("missing inline=1")
	}
	if !strings.Contains(seq, "size=") {
		t.Error("missing size parameter")
	}
	if !strings.Contains(seq, "width=4;") {
		t.Error("missing width=4")
	}
	if !strings.Contains(seq, "height=2:") {
		t.Error("missing height=2")
	}
}
