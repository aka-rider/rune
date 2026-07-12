package markdownedit

import (
	"fmt"
	"image/color"

	"charm.land/lipgloss/v2"

	"rune/pkg/imagekit"
	"rune/pkg/ui/components/textedit"
)

// imagePlaceholderCells builds cells for one reserved image row using the
// Kitty Unicode-placeholder virtual-placement scheme.
func imagePlaceholderCells(id uint32, rowIndex, cols int) []textedit.Cell {
	style := lipgloss.NewStyle().Foreground(idToColor(id))
	placeholder := string(imagekit.Placeholder)
	rowDiacritic := string(imagekit.Diacritic(rowIndex))

	cells := make([]textedit.Cell, 0, cols)
	for col := 0; col < cols; col++ {
		cells = append(cells, textedit.Cell{
			Grapheme:  placeholder + rowDiacritic + string(imagekit.Diacritic(col)),
			Width:     1,
			Style:     style,
			BufOffset: -1,
		})
	}
	return cells
}

// idToColor maps a 24-bit image ID to a truecolor value.
//
// This is Kitty image-ID ENCODING (the placeholder cell's foreground color
// carries the protocol's image ID), not theming — it must not be routed
// through styles.Palette/Styles tokens.
func idToColor(id uint32) color.Color {
	return lipgloss.Color(fmt.Sprintf("#%06X", id&0xFFFFFF))
}
