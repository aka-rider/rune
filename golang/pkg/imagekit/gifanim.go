package imagekit

import (
	"bytes"
	"errors"
	"fmt"
	"image"
	"image/draw"
	"image/gif"
	"time"
)

// ErrNotAnimated is returned by DecodeGIF when the data is a single-frame GIF.
var ErrNotAnimated = errors.New("gif is not animated")

// minFrameDelay is the floor applied to GIF frame delays; many GIFs encode a
// 0/very-small delay expecting the renderer to clamp.
const minFrameDelay = 50 * time.Millisecond

// Animation holds the fully-composited frames of an animated GIF. Each frame is
// a complete RGBA image of the logical screen (disposal already applied), ready
// to transmit independently.
type Animation struct {
	Frames    []*image.RGBA
	Delays    []time.Duration
	LoopCount int // 0 = loop forever, <0 = play once, >0 = explicit loop count
	Width     int
	Height    int
}

// ClampDelay converts a GIF delay (hundredths of a second) to a duration,
// flooring it at minFrameDelay.
func ClampDelay(hundredths int) time.Duration {
	d := time.Duration(hundredths) * 10 * time.Millisecond
	if d < minFrameDelay {
		return minFrameDelay
	}
	return d
}

// DecodeGIF decodes an animated GIF, compositing each frame onto a persistent
// canvas while honoring per-frame disposal methods (None/Background/Previous).
// It returns ErrNotAnimated for single-frame GIFs so callers can fall back to a
// still decode.
func DecodeGIF(data []byte) (*Animation, error) {
	g, err := gif.DecodeAll(bytes.NewReader(data))
	if err != nil {
		return nil, fmt.Errorf("decode gif: %w", err)
	}
	if len(g.Image) <= 1 {
		return nil, ErrNotAnimated
	}

	w, h := g.Config.Width, g.Config.Height
	if w <= 0 || h <= 0 {
		b := g.Image[0].Bounds()
		w, h = b.Dx(), b.Dy()
	}
	bounds := image.Rect(0, 0, w, h)

	frames := make([]*image.RGBA, len(g.Image))
	delays := make([]time.Duration, len(g.Image))
	canvas := image.NewRGBA(bounds)

	for i, src := range g.Image {
		var backup *image.RGBA
		disposal := disposalAt(g, i)
		if disposal == gif.DisposalPrevious {
			backup = cloneRGBA(canvas)
		}

		draw.Draw(canvas, src.Bounds(), src, src.Bounds().Min, draw.Over)
		frames[i] = cloneRGBA(canvas)
		delays[i] = ClampDelay(delayAt(g, i))

		switch disposal {
		case gif.DisposalBackground:
			clearRect(canvas, src.Bounds())
		case gif.DisposalPrevious:
			canvas = backup
		}
	}

	return &Animation{
		Frames:    frames,
		Delays:    delays,
		LoopCount: g.LoopCount,
		Width:     w,
		Height:    h,
	}, nil
}

func disposalAt(g *gif.GIF, i int) byte {
	if i < len(g.Disposal) {
		return g.Disposal[i]
	}
	return gif.DisposalNone
}

func delayAt(g *gif.GIF, i int) int {
	if i < len(g.Delay) {
		return g.Delay[i]
	}
	return 0
}

func cloneRGBA(src *image.RGBA) *image.RGBA {
	dst := image.NewRGBA(src.Bounds())
	copy(dst.Pix, src.Pix)
	return dst
}

func clearRect(img *image.RGBA, r image.Rectangle) {
	draw.Draw(img, r, image.Transparent, image.Point{}, draw.Src)
}
