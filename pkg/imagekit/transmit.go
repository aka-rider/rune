package imagekit

import (
	"bytes"
	"encoding/base64"
	"fmt"
	"hash/fnv"
	"image"
	"image/png"
	"strings"

	"github.com/charmbracelet/x/ansi/kitty"
)

// Verified Kitty Graphics protocol API (github.com/charmbracelet/x/ansi
// v0.11.7, package .../kitty), confirmed against module source:
//
//   - func EncodeGraphics(w io.Writer, m image.Image, o *Options) error
//     writes the full APC sequence, base64-encoding and chunking when
//     o.Chunk is true (chunks of kitty.MaxChunkSize = 4096 bytes).
//   - Options fields used here: Action byte, Quite byte (NOTE: spelled
//     "Quite", not "Quiet"), ID int, Format int, Compression byte, Columns
//     int, Rows int, VirtualPlacement bool, Chunk bool, Delete byte,
//     DeleteResources bool.
//   - Action consts: Transmit ('t'), TransmitAndPut ('T'), Query ('q'),
//     Put ('p'), Delete ('d'), Frame ('f'), Animate ('a'), Compose ('c').
//   - Format consts: RGBA (32), RGB (24), PNG (100).
//   - Compression: Zlib ('z').
//   - Delete consts: DeleteAll ('a'), DeleteID ('i'), DeleteFrames ('f').
//   - const Placeholder = '\U0010EEEE'; func Diacritic(i int) rune.
//
// Unicode virtual placement (U=1) is used so placeholder cells flow through the
// normal cell-grid renderer. The image ID is carried in each placeholder cell's
// foreground color (see the editor's render layer), so IDs are masked to 24
// bits by AllocID.

// idMask24 bounds an image ID to 24 bits so it fits a single truecolor value.
const idMask24 = 0xFFFFFF

// Placeholder is the Unicode placeholder rune (U+10EEEE) for virtual placement.
const Placeholder = kitty.Placeholder

// Diacritic returns the row/column-encoding combining diacritic for index i.
func Diacritic(i int) rune { return kitty.Diacritic(i) }

// AllocID derives a deterministic, non-zero, 24-bit image ID from an absolute
// path using fnv32a. The 24-bit bound lets the ID be encoded in a single
// truecolor foreground value for Unicode placeholder cells.
func AllocID(absPath string) uint32 {
	h := fnv.New32a()
	_, _ = h.Write([]byte(absPath))
	id := h.Sum32() & idMask24
	if id == 0 {
		id = 1
	}
	return id
}

// EncodeTransmit encodes img for transmit-and-display via Unicode virtual
// placement, scaled to cols x rows terminal cells. The returned string is an
// APC escape sequence (possibly several, when chunked) ready to be written to
// the tty. The image is transmitted as PNG.
func EncodeTransmit(img image.Image, id uint32, cols, rows int) (string, error) {
	var b strings.Builder
	opts := &kitty.Options{
		Action:           kitty.TransmitAndPut,
		Transmission:     kitty.Direct,
		VirtualPlacement: true,
		ID:               int(id),
		Format:           kitty.PNG,
		Columns:          cols,
		Rows:             rows,
		Quite:            2, // suppress OK and error responses
		Chunk:            true,
	}
	if err := kitty.EncodeGraphics(&b, img, opts); err != nil {
		return "", err
	}
	return b.String(), nil
}

// EncodeDelete returns an APC sequence that deletes the image with the given ID
// and frees its data from the terminal.
func EncodeDelete(id uint32) string {
	var b strings.Builder
	_ = kitty.EncodeGraphics(&b, nil, &kitty.Options{
		Action:          kitty.Delete,
		Delete:          kitty.DeleteID,
		ID:              int(id),
		DeleteResources: true,
		Quite:           2,
	})
	return b.String()
}

// EncodeDeleteAll returns an APC sequence that deletes all images and frees
// their data from the terminal.
func EncodeDeleteAll() string {
	var b strings.Builder
	_ = kitty.EncodeGraphics(&b, nil, &kitty.Options{
		Action:          kitty.Delete,
		Delete:          kitty.DeleteAll,
		DeleteResources: true,
		Quite:           2,
	})
	return b.String()
}

// EncodeITerm2 encodes img as a PNG and wraps it in an iTerm2 inline image OSC
// 1337 escape sequence. The escape is self-contained: no prior transmit or ID
// assignment is needed. The terminal renders the image at the cursor position,
// scaling to fit the provided cell dimensions.
//
// Format: \033]1337;File=inline=1;size=N;width={cols};height={rows}:{base64png}\a
func EncodeITerm2(img image.Image, cols, rows int) (string, error) {
	var buf bytes.Buffer
	if err := png.Encode(&buf, img); err != nil {
		return "", fmt.Errorf("encode png for iterm2: %w", err)
	}
	payload := base64.StdEncoding.EncodeToString(buf.Bytes())
	var sb strings.Builder
	sb.WriteString("\033]1337;File=inline=1;")
	sb.WriteString(fmt.Sprintf("size=%d;", buf.Len()))
	sb.WriteString(fmt.Sprintf("width=%d;height=%d:", cols, rows))
	sb.WriteString(payload)
	sb.WriteByte('\a')
	return sb.String(), nil
}

// EncodeITerm2Rows slices img into independent 1-row-tall strips and encodes
// each as a separate OSC 1337 payload. Each slice is cols cells wide × 1 cell
// tall, containing the pixels for that row of the image. This enables
// viewport-clipped placement: only the visible row-slices are written to the
// TTY, preventing vertical overflow.
func EncodeITerm2Rows(img image.Image, cols, rows int, cs CellSize) ([]string, error) {
	bounds := img.Bounds()
	imgH := bounds.Dy()
	if rows <= 0 || imgH <= 0 {
		return nil, nil
	}

	slices := make([]string, rows)
	rowPixH := imgH / rows // pixel height per row-slice
	if rowPixH < 1 {
		rowPixH = 1
	}

	for r := 0; r < rows; r++ {
		y0 := bounds.Min.Y + r*rowPixH
		y1 := y0 + rowPixH
		if r == rows-1 {
			// Last row gets any remaining pixels (avoids rounding gaps).
			y1 = bounds.Max.Y
		}
		if y1 > bounds.Max.Y {
			y1 = bounds.Max.Y
		}

		strip := img.(interface {
			SubImage(r image.Rectangle) image.Image
		}).SubImage(image.Rect(bounds.Min.X, y0, bounds.Max.X, y1))

		var buf bytes.Buffer
		if err := png.Encode(&buf, strip); err != nil {
			return nil, fmt.Errorf("encode iterm2 row %d: %w", r, err)
		}
		payload := base64.StdEncoding.EncodeToString(buf.Bytes())
		var sb strings.Builder
		sb.WriteString("\033]1337;File=inline=1;")
		sb.WriteString(fmt.Sprintf("size=%d;", buf.Len()))
		sb.WriteString(fmt.Sprintf("width=%d;height=1;preserveAspectRatio=0:", cols))
		sb.WriteString(payload)
		sb.WriteByte('\a')
		slices[r] = sb.String()
	}
	return slices, nil
}
