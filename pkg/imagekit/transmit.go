package imagekit

import (
	"hash/fnv"
	"image"
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
