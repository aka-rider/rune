package imagekit

import (
	"bytes"
	"fmt"
	"image"
	"math"

	"github.com/srwiley/oksvg"
	"github.com/srwiley/rasterx"
)

// maxSVGDimension caps the intrinsic raster dimension produced for an SVG, to
// bound memory use on hostile or pathological input.
const maxSVGDimension = 4096

// rasterizeSVG parses an SVG document and rasterizes it to an *image.RGBA at
// its intrinsic (viewBox) dimensions, clamped to maxSVGDimension. It never
// panics — DecodeStill's recover guards the underlying parser as well, but we
// also clamp dimensions defensively here.
func rasterizeSVG(data []byte) (Decoded, error) {
	icon, err := oksvg.ReadIconStream(bytes.NewReader(data), oksvg.IgnoreErrorMode)
	if err != nil {
		return Decoded{}, fmt.Errorf("parse svg: %w", err)
	}

	w := int(math.Ceil(icon.ViewBox.W))
	h := int(math.Ceil(icon.ViewBox.H))
	if w <= 0 || h <= 0 {
		// No usable viewBox — fall back to a square default canvas.
		w, h = 512, 512
	}
	if w > maxSVGDimension {
		w = maxSVGDimension
	}
	if h > maxSVGDimension {
		h = maxSVGDimension
	}

	icon.SetTarget(0, 0, float64(w), float64(h))

	rgba := image.NewRGBA(image.Rect(0, 0, w, h))
	scanner := rasterx.NewScannerGV(w, h, rgba, rgba.Bounds())
	dasher := rasterx.NewDasher(w, h, scanner)
	icon.Draw(dasher, 1.0)

	return Decoded{
		Image:  rgba,
		Width:  w,
		Height: h,
		Format: "svg",
	}, nil
}
