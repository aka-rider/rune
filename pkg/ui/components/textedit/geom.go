package textedit

import "rune/pkg/editor/display"

// Geom is one coherent read of a Model's render geometry — the display
// snapshot, syntax snapshot, viewport, and screen placement, captured from
// the SAME Model value in a single call. It exists for consumers that need a
// mutually consistent view across several of these fields at once (mouse
// hit-testing, image placement math), replacing the individual
// Snapshot/SyntaxSnap/Viewport/OffsetX/OffsetY accessors that existed only
// for this kind of markdownedit reach-in (§12 — RenderView/CellBuilderFunc
// remain the separate render seam; Geom is a plain data read, not an
// extension point).
type Geom struct {
	Snap          display.DisplaySnapshot
	Syntax        display.SyntaxSnapshot
	Viewport      ViewportState
	OffsetX       int
	OffsetY       int
	Width         int
	ContentHeight int
}

// Geom returns one coherent read of the Model's render geometry.
func (m Model) Geom() Geom {
	return Geom{
		Snap:          m.snapshot,
		Syntax:        m.syntaxSnap,
		Viewport:      m.viewport,
		OffsetX:       m.offsetX,
		OffsetY:       m.offsetY,
		Width:         m.width,
		ContentHeight: m.contentHeight(),
	}
}

// ImageMaxCols returns the maximum column width for rendered images, floored
// at 1. Same formula as the retired Model.ImageMaxCols.
func (g Geom) ImageMaxCols() int {
	w := g.Width - 2
	if w < 1 {
		return 1
	}
	return w
}
