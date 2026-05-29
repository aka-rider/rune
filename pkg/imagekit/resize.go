package imagekit

import (
	"image"

	"golang.org/x/image/draw"
)

// FitBox returns the largest (w, h) that preserves the source aspect ratio and
// fits within maxW x maxH. It never upscales beyond the source dimensions and
// never returns a dimension below 1 (for a non-degenerate source).
func FitBox(srcW, srcH, maxW, maxH int) (w, h int) {
	if srcW <= 0 || srcH <= 0 || maxW <= 0 || maxH <= 0 {
		return 0, 0
	}
	if srcW <= maxW && srcH <= maxH {
		return srcW, srcH
	}
	// Scale down by the more constraining ratio.
	sw := float64(maxW) / float64(srcW)
	sh := float64(maxH) / float64(srcH)
	s := sw
	if sh < s {
		s = sh
	}
	w = int(float64(srcW) * s)
	h = int(float64(srcH) * s)
	if w < 1 {
		w = 1
	}
	if h < 1 {
		h = 1
	}
	return w, h
}

// Resize scales src to exactly w x h using a high-quality CatmullRom kernel and
// returns a fresh *image.RGBA. If w or h is non-positive, it returns an empty
// 1x1 image to avoid panics in downstream encoders.
func Resize(src image.Image, w, h int) *image.RGBA {
	if w < 1 || h < 1 {
		return image.NewRGBA(image.Rect(0, 0, 1, 1))
	}
	dst := image.NewRGBA(image.Rect(0, 0, w, h))
	draw.CatmullRom.Scale(dst, dst.Bounds(), src, src.Bounds(), draw.Over, nil)
	return dst
}
