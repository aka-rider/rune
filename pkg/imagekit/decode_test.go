package imagekit

import (
	"bytes"
	"image"
	"image/color"
	"image/gif"
	"image/jpeg"
	"image/png"
	"testing"

	"golang.org/x/image/bmp"
	"golang.org/x/image/tiff"
)

// solidImage returns a w x h image filled with a single opaque color.
func solidImage(w, h int, c color.Color) *image.RGBA {
	img := image.NewRGBA(image.Rect(0, 0, w, h))
	for y := 0; y < h; y++ {
		for x := 0; x < w; x++ {
			img.Set(x, y, c)
		}
	}
	return img
}

func TestDecodeStill_Formats(t *testing.T) {
	src := solidImage(7, 5, color.RGBA{R: 10, G: 200, B: 30, A: 255})

	encoders := map[string]func() []byte{
		"png": func() []byte {
			var b bytes.Buffer
			_ = png.Encode(&b, src)
			return b.Bytes()
		},
		"jpeg": func() []byte {
			var b bytes.Buffer
			_ = jpeg.Encode(&b, src, nil)
			return b.Bytes()
		},
		"gif": func() []byte {
			var b bytes.Buffer
			_ = gif.Encode(&b, src, nil)
			return b.Bytes()
		},
		"bmp": func() []byte {
			var b bytes.Buffer
			_ = bmp.Encode(&b, src)
			return b.Bytes()
		},
		"tiff": func() []byte {
			var b bytes.Buffer
			_ = tiff.Encode(&b, src, nil)
			return b.Bytes()
		},
	}

	for format, enc := range encoders {
		t.Run(format, func(t *testing.T) {
			data := enc()
			d, err := DecodeStill(data)
			if err != nil {
				t.Fatalf("DecodeStill(%s): %v", format, err)
			}
			if d.Width != 7 || d.Height != 5 {
				t.Errorf("%s: got %dx%d, want 7x5", format, d.Width, d.Height)
			}
			if d.Image == nil {
				t.Errorf("%s: nil image", format)
			}
		})
	}
}

const tinySVG = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 16">` +
	`<rect x="0" y="0" width="24" height="16" fill="#3366ff"/></svg>`

func TestDecodeStill_SVG(t *testing.T) {
	d, err := DecodeStill([]byte(tinySVG))
	if err != nil {
		t.Fatalf("DecodeStill(svg): %v", err)
	}
	if d.Format != "svg" {
		t.Errorf("format=%q, want svg", d.Format)
	}
	if d.Width != 24 || d.Height != 16 {
		t.Errorf("got %dx%d, want 24x16", d.Width, d.Height)
	}
}

func TestDecodeStill_EmptyAndGarbage(t *testing.T) {
	if _, err := DecodeStill(nil); err == nil {
		t.Error("expected error for empty data")
	}
	if _, err := DecodeStill([]byte("not an image at all")); err == nil {
		t.Error("expected error for garbage data")
	}
}

func TestSniffFormat(t *testing.T) {
	cases := []struct {
		name string
		data []byte
		want string
	}{
		{"svg", []byte(tinySVG), "svg"},
		{"svg-with-xml-decl", []byte(`<?xml version="1.0"?>` + tinySVG), "svg"},
		{"png", func() []byte { var b bytes.Buffer; _ = png.Encode(&b, solidImage(2, 2, color.White)); return b.Bytes() }(), "png"},
		{"garbage", []byte("zzzz"), ""},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			if got := SniffFormat(tc.data); got != tc.want {
				t.Errorf("SniffFormat=%q, want %q", got, tc.want)
			}
		})
	}
}
