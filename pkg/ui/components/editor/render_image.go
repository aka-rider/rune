package editor

import (
	"fmt"
	"image/color"

	"charm.land/lipgloss/v2"

	"rune/pkg/editor/display"
	"rune/pkg/imagekit"
)

// imageKittyCapable reports whether inline image rendering is active for this
// terminal (Kitty/Ghostty with truecolor).
func (m Model) imageKittyCapable() bool {
	return m.termCaps.SupportsKittyGraphics()
}

// imageInlineCapable reports whether the terminal supports iTerm2 inline images
// (OSC 1337). WezTerm and iTerm2 qualify.
func (m Model) imageInlineCapable() bool {
	return m.termCaps.SupportsInlineImages()
}

// imageCapable reports whether any image rendering path is available.
func (m Model) imageCapable() bool {
	return m.imageKittyCapable() || m.imageInlineCapable()
}

// imageDimsFor is the callback handed to display.ExpandImageRows. It reserves
// rows once an image's dimensions are known (pendingTransmit or live) on a
// capable terminal; pendingDecode and failed entries stay a single row and fall
// back to alt text. This is a pure read of registry metadata — no decode, no
// I/O.
func (m Model) imageDimsFor(path string) display.ImageDims {
	if !m.imageCapable() {
		return display.ImageDims{Cols: 0, Rows: 1}
	}
	e, ok := m.images.get(path)
	if !ok || e.rows <= 0 {
		return display.ImageDims{Cols: 0, Rows: 1}
	}
	if e.state != pendingTransmit && e.state != live {
		return display.ImageDims{Cols: 0, Rows: 1}
	}
	return display.ImageDims{Cols: e.cols, Rows: e.rows}
}

// imageIDFor returns the Kitty ID used to render placeholder cells for a path.
// It prefers the registry's assigned ID and falls back to a deterministic
// derivation (used by render tests with a pre-expanded snapshot and no
// registry entry).
func (m Model) imageIDFor(path string) uint32 {
	if e, ok := m.images.get(path); ok {
		if e.animated && len(e.frameIDs) > 0 {
			idx := e.frameIdx
			if idx < 0 || idx >= len(e.frameIDs) {
				idx = 0
			}
			return e.frameIDs[idx]
		}
		if e.id != 0 {
			return e.id
		}
	}
	return imagekit.AllocID(path)
}

// imagePlaceholderCells builds the cells for one reserved image row using the
// Kitty Unicode-placeholder virtual-placement scheme: every cell holds the
// placeholder rune U+10EEEE followed by a row diacritic and a column diacritic,
// and carries the image ID in its foreground color. The cells are decorative
// (BufOffset -1) so cursor/selection overlays never touch them, and each is
// width 1 so the row occupies exactly `cols` terminal cells.
func imagePlaceholderCells(id uint32, rowIndex, cols int) []Cell {
	style := lipgloss.NewStyle().Foreground(idToColor(id))
	placeholder := string(imagekit.Placeholder)
	rowDiacritic := string(imagekit.Diacritic(rowIndex))

	cells := make([]Cell, 0, cols)
	for col := 0; col < cols; col++ {
		cells = append(cells, Cell{
			Grapheme:  placeholder + rowDiacritic + string(imagekit.Diacritic(col)),
			Width:     1,
			Style:     style,
			BufOffset: -1,
		})
	}
	return cells
}

// idToColor maps a 24-bit image ID to a truecolor value whose RGB bytes equal
// the ID. The terminal reads this foreground color back to bind the placeholder
// cells to the transmitted image.
func idToColor(id uint32) color.Color {
	return lipgloss.Color(fmt.Sprintf("#%06X", id&0xFFFFFF))
}
