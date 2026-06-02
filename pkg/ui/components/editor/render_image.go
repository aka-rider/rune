package editor

import (
	"fmt"
	"image/color"

	"charm.land/lipgloss/v2"

	"rune/pkg/imagekit"
)

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
