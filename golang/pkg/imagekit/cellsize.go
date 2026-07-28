package imagekit

// CellSize is the pixel size of a single terminal cell.
type CellSize struct {
	W, H int
}

// DefaultCellSize returns the conventional 8x16 fallback used when the terminal
// does not report its cell pixel dimensions. Aspect ratio may be slightly off
// on terminals with a different cell geometry, but rendering is never broken.
func DefaultCellSize() CellSize {
	return CellSize{W: 8, H: 16}
}

// FitCells computes how many terminal columns and rows an image of pxW x pxH
// pixels should occupy, preserving aspect ratio and fitting within
// maxCols x maxRows. Both results are at least 1 for a non-degenerate image.
func FitCells(pxW, pxH, maxCols, maxRows int, cs CellSize) (cols, rows int) {
	if pxW <= 0 || pxH <= 0 || cs.W <= 0 || cs.H <= 0 {
		return 0, 0
	}
	if maxCols < 1 {
		maxCols = 1
	}
	if maxRows < 1 {
		maxRows = 1
	}

	// Natural cell footprint, rounding up so the image is never clipped.
	cols = ceilDiv(pxW, cs.W)
	rows = ceilDiv(pxH, cs.H)

	// Scale down preserving aspect if it exceeds the allowed box.
	if cols > maxCols || rows > maxRows {
		sw := float64(maxCols) / float64(cols)
		sh := float64(maxRows) / float64(rows)
		s := sw
		if sh < s {
			s = sh
		}
		cols = int(float64(cols) * s)
		rows = int(float64(rows) * s)
	}

	if cols < 1 {
		cols = 1
	}
	if rows < 1 {
		rows = 1
	}
	return cols, rows
}

func ceilDiv(a, b int) int {
	if b <= 0 {
		return 0
	}
	return (a + b - 1) / b
}
