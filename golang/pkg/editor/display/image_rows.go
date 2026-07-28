package display

import "strings"

// ImageDims is the cell footprint a standalone image should occupy, as reported
// by the dimsFor callback passed to ExpandImageRows.
type ImageDims struct {
	Cols, Rows int
}

// ExpandImageRows returns a new snapshot in which every qualifying standalone
// image line (see isStandaloneImageLine) is replaced by ImageDims.Rows reserved
// display rows: an anchor row carrying the original spans plus image metadata,
// followed by empty continuation rows that share the same ModelLine. The
// private row/line index arrays are rebuilt to match.
//
// This is a pure function: it never decodes or performs I/O — all sizing comes
// from the injected dimsFor callback. When dimsFor reports Rows <= 1 for every
// image (or there are no qualifying image lines), the input snapshot is
// returned unchanged.
func ExpandImageRows(ds DisplaySnapshot, dimsFor func(imagePath string) ImageDims) DisplaySnapshot {
	if dimsFor == nil {
		return ds
	}

	newLines := make([]DisplayLine, 0, len(ds.Lines))
	expanded := false

	for _, l := range ds.Lines {
		path, ok := isStandaloneImageLine(l)
		if ok {
			dims := dimsFor(path)
			if dims.Rows > 1 {
				expanded = true

				// anchor is a full struct copy of l, so anchor.WrapRow already
				// carries the source row's true wrap-space index — only the
				// image-reservation fields below are overwritten.
				anchor := l
				anchor.ImagePath = path
				anchor.ImageRowIndex = 0
				anchor.ImageRowCount = dims.Rows
				anchor.ImageCols = dims.Cols
				newLines = append(newLines, anchor)

				for r := 1; r < dims.Rows; r++ {
					newLines = append(newLines, DisplayLine{
						Spans:         nil,
						ModelLine:     l.ModelLine,
						WrapRow:       l.WrapRow,
						ImagePath:     path,
						ImageRowIndex: r,
						ImageRowCount: dims.Rows,
						ImageCols:     dims.Cols,
					})
				}
				continue
			}
		}
		newLines = append(newLines, l)
	}

	if !expanded {
		return ds
	}

	rowToModelLine := make([]int, len(newLines))
	lineToFirstRow := make([]int, len(ds.lineToFirstRow))
	seen := make([]bool, len(ds.lineToFirstRow))
	for r, l := range newLines {
		rowToModelLine[r] = l.ModelLine
		if l.ModelLine >= 0 && l.ModelLine < len(lineToFirstRow) && !seen[l.ModelLine] {
			lineToFirstRow[l.ModelLine] = r
			seen[l.ModelLine] = true
		}
	}

	return DisplaySnapshot{
		Lines:          newLines,
		TotalRows:      len(newLines),
		rowToModelLine: rowToModelLine,
		lineToFirstRow: lineToFirstRow,
	}
}

// StandaloneImagePath reports the image path if the line qualifies as a
// standalone image line (see isStandaloneImageLine), for callers that discover
// renderable images without triggering row expansion.
func StandaloneImagePath(l DisplayLine) (string, bool) {
	return isStandaloneImageLine(l)
}

// isStandaloneImageLine reports whether the line consists of exactly one
// Rendered image span, optionally preceded by leading whitespace-only text and
// a single list marker. Trailing whitespace-only text is tolerated. Any other
// substantive span (including text adjacent to the image, or a Revealed image)
// disqualifies the line, so truly-inline images keep the alt-text fallback.
func isStandaloneImageLine(l DisplayLine) (string, bool) {
	var imgPath string
	found := false
	listMarkerSeen := false

	for _, sp := range l.Spans {
		switch {
		case sp.Kind == TokenText && strings.TrimSpace(sp.Text) == "":
			// Leading/trailing whitespace — ignore.
			continue
		case sp.Kind == TokenListMarker && !listMarkerSeen && !found:
			listMarkerSeen = true
			continue
		case sp.Kind == TokenImage || (sp.Kind == TokenWikiLink && sp.WikiLinkIsImage):
			if found || sp.State != Rendered || sp.ImagePath == "" {
				return "", false
			}
			imgPath = sp.ImagePath
			found = true
			continue
		default:
			return "", false
		}
	}

	return imgPath, found
}
