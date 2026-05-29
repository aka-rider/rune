// Package imagekit is a pure image foundation for inline terminal rendering:
// it decodes, measures, resizes, and encodes images for the Kitty graphics
// protocol. It MUST NOT import bubbletea, the editor, the display layer, or
// pkg/terminal — it deals only in image bytes, pixels, and escape-sequence
// strings.
package imagekit

import (
	"bytes"
	"errors"
	"fmt"
	"image"
	"strings"

	// Register standard-library still-image decoders.
	_ "image/gif"
	_ "image/jpeg"
	_ "image/png"

	// Register golang.org/x/image decoders.
	_ "golang.org/x/image/bmp"
	_ "golang.org/x/image/tiff"
	_ "golang.org/x/image/webp"
)

// Decoded is the result of decoding a single still image.
type Decoded struct {
	Image  image.Image
	Width  int
	Height int
	Format string // "png", "jpeg", "gif", "bmp", "tiff", "webp", "svg"
}

// SniffFormat inspects the leading bytes of data and reports a best-effort
// format name. It routes SVG/XML prefixes to "svg" and otherwise defers to the
// standard image format registry. Returns "" when the format is unknown.
func SniffFormat(data []byte) string {
	if looksLikeSVG(data) {
		return "svg"
	}
	_, format, err := image.DecodeConfig(bytes.NewReader(data))
	if err != nil {
		return ""
	}
	return format
}

// looksLikeSVG reports whether data appears to be an SVG document. It tolerates
// a leading UTF-8 BOM, whitespace, an XML declaration, and DOCTYPE/comment
// preamble before the root <svg> element.
func looksLikeSVG(data []byte) bool {
	s := strings.TrimSpace(string(bytes.TrimPrefix(data, []byte("\xef\xbb\xbf"))))
	if s == "" {
		return false
	}
	low := strings.ToLower(s)
	if strings.HasPrefix(low, "<svg") {
		return true
	}
	// XML prologue / doctype / comment before the root element.
	if strings.HasPrefix(low, "<?xml") || strings.HasPrefix(low, "<!doctype") || strings.HasPrefix(low, "<!--") {
		return strings.Contains(low, "<svg")
	}
	return false
}

// DecodeStill decodes data into a single still image. SVG input is rasterized.
// It never panics: a panic inside an underlying decoder is recovered and
// returned as an error, because image decoders are exposed directly to
// untrusted document content.
func DecodeStill(data []byte) (_ Decoded, err error) {
	if len(data) == 0 {
		return Decoded{}, errors.New("decode image: empty data")
	}

	defer func() {
		if r := recover(); r != nil {
			err = fmt.Errorf("decode image: decoder panicked: %v", r)
		}
	}()

	if looksLikeSVG(data) {
		d, rerr := rasterizeSVG(data)
		if rerr != nil {
			return Decoded{}, fmt.Errorf("decode svg image: %w", rerr)
		}
		return d, nil
	}

	img, format, derr := image.Decode(bytes.NewReader(data))
	if derr != nil {
		return Decoded{}, fmt.Errorf("decode %s image: %w", fallbackFormat(format), derr)
	}
	b := img.Bounds()
	return Decoded{
		Image:  img,
		Width:  b.Dx(),
		Height: b.Dy(),
		Format: format,
	}, nil
}

func fallbackFormat(format string) string {
	if format == "" {
		return "unknown"
	}
	return format
}
