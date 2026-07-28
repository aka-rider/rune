package imagekit

import (
	"bytes"
	"errors"
	"image"
	"image/color"
	"image/gif"
	"testing"
	"time"
)

// animatedGIF builds a 2-frame 2x1 GIF using palette transparency so disposal
// compositing can be verified. Frame 0 paints (0,0)=red; frame 1 paints
// (1,0)=blue with (0,0) transparent. With DisposalNone the red must persist.
func animatedGIF(t *testing.T, disposal byte) []byte {
	t.Helper()
	transparent := color.RGBA{0, 0, 0, 0}
	red := color.RGBA{255, 0, 0, 255}
	blue := color.RGBA{0, 0, 255, 255}
	pal := color.Palette{transparent, red, blue}

	f0 := image.NewPaletted(image.Rect(0, 0, 2, 1), pal)
	f0.SetColorIndex(0, 0, 1) // red
	f0.SetColorIndex(1, 0, 0) // transparent

	f1 := image.NewPaletted(image.Rect(0, 0, 2, 1), pal)
	f1.SetColorIndex(0, 0, 0) // transparent (keep prior)
	f1.SetColorIndex(1, 0, 2) // blue

	g := &gif.GIF{
		Image:     []*image.Paletted{f0, f1},
		Delay:     []int{10, 10},
		Disposal:  []byte{disposal, disposal},
		LoopCount: 0,
		Config:    image.Config{ColorModel: pal, Width: 2, Height: 1},
	}
	var b bytes.Buffer
	if err := gif.EncodeAll(&b, g); err != nil {
		t.Fatal(err)
	}
	return b.Bytes()
}

func TestDecodeGIF_FrameCountAndDisposalNone(t *testing.T) {
	data := animatedGIF(t, gif.DisposalNone)
	anim, err := DecodeGIF(data)
	if err != nil {
		t.Fatalf("DecodeGIF: %v", err)
	}
	if len(anim.Frames) != 2 {
		t.Fatalf("frame count=%d, want 2", len(anim.Frames))
	}
	if anim.Width != 2 || anim.Height != 1 {
		t.Errorf("dims=%dx%d, want 2x1", anim.Width, anim.Height)
	}

	// Frame 0: red at (0,0).
	if r, _, _, a := anim.Frames[0].At(0, 0).RGBA(); r>>8 != 255 || a>>8 != 255 {
		t.Errorf("frame0 (0,0) not opaque red: %v", anim.Frames[0].At(0, 0))
	}
	// Frame 1 with DisposalNone: red persists at (0,0), blue at (1,0).
	if r, _, _, a := anim.Frames[1].At(0, 0).RGBA(); r>>8 != 255 || a>>8 != 255 {
		t.Errorf("frame1 (0,0) should keep composited red, got %v", anim.Frames[1].At(0, 0))
	}
	if _, _, b, a := anim.Frames[1].At(1, 0).RGBA(); b>>8 != 255 || a>>8 != 255 {
		t.Errorf("frame1 (1,0) should be blue, got %v", anim.Frames[1].At(1, 0))
	}
}

func TestDecodeGIF_DisposalBackgroundClears(t *testing.T) {
	data := animatedGIF(t, gif.DisposalBackground)
	anim, err := DecodeGIF(data)
	if err != nil {
		t.Fatalf("DecodeGIF: %v", err)
	}
	// With DisposalBackground, frame 0's red rect is cleared before frame 1, so
	// frame 1 must NOT show red at (0,0).
	if r, _, _, a := anim.Frames[1].At(0, 0).RGBA(); r>>8 == 255 && a>>8 == 255 {
		t.Errorf("frame1 (0,0) should be cleared, but shows red: %v", anim.Frames[1].At(0, 0))
	}
}

func TestDecodeGIF_SingleFrameNotAnimated(t *testing.T) {
	pal := color.Palette{color.RGBA{1, 2, 3, 255}}
	f := image.NewPaletted(image.Rect(0, 0, 2, 2), pal)
	g := &gif.GIF{Image: []*image.Paletted{f}, Delay: []int{0}, Config: image.Config{ColorModel: pal, Width: 2, Height: 2}}
	var b bytes.Buffer
	if err := gif.EncodeAll(&b, g); err != nil {
		t.Fatal(err)
	}
	if _, err := DecodeGIF(b.Bytes()); !errors.Is(err, ErrNotAnimated) {
		t.Errorf("expected ErrNotAnimated, got %v", err)
	}
}

func TestClampDelay(t *testing.T) {
	if d := ClampDelay(0); d != 50*time.Millisecond {
		t.Errorf("ClampDelay(0)=%v, want 50ms", d)
	}
	if d := ClampDelay(3); d != 50*time.Millisecond {
		t.Errorf("ClampDelay(3)=%v, want clamped 50ms", d)
	}
	if d := ClampDelay(10); d != 100*time.Millisecond {
		t.Errorf("ClampDelay(10)=%v, want 100ms", d)
	}
}
