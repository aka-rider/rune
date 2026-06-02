package display

import (
	"unicode/utf8"

	"rune/pkg/editor/coords"

	"github.com/mattn/go-runewidth"
)

type WrapSegment struct {
	Spans     []SyntaxSpan
	ModelLine int
	WrapIndex int
	StartCol  int // in syntax space bytes
}

type WrapSnapshot struct {
	Segments       []WrapSegment
	TotalRows      int
	rowToSegment   []int
	lineToFirstRow []int
}

func (w WrapSnapshot) SyntaxToWrap(sp coords.SyntaxPoint) coords.WrapPoint {
	if sp.Line < 0 || sp.Line >= len(w.lineToFirstRow) {
		return coords.WrapPoint{Row: 0, Col: 0}
	}
	firstRow := w.lineToFirstRow[sp.Line]

	// Track the last segment of this line for fallback clamping
	targetRow := firstRow
	wrapCol := 0
	lastSegRow := firstRow
	lastSegLen := 0

	for i := firstRow; i < len(w.Segments) && w.Segments[i].ModelLine == sp.Line; i++ {
		seg := w.Segments[i]

		segLen := 0
		for _, span := range seg.Spans {
			segLen += len(span.Text)
		}

		lastSegRow = i
		lastSegLen = segLen

		// Use strict < for upper bound: positions at segment boundary prefer next segment.
		// Exception: the last segment of the line uses <= (no next segment to prefer).
		nextIssameLine := i+1 < len(w.Segments) && w.Segments[i+1].ModelLine == sp.Line
		upperInclusive := !nextIssameLine

		if upperInclusive {
			if sp.Col >= seg.StartCol && sp.Col <= seg.StartCol+segLen {
				targetRow = i
				wrapCol = sp.Col - seg.StartCol
				return coords.WrapPoint{Row: targetRow, Col: wrapCol}
			}
		} else {
			if sp.Col >= seg.StartCol && sp.Col < seg.StartCol+segLen {
				targetRow = i
				wrapCol = sp.Col - seg.StartCol
				return coords.WrapPoint{Row: targetRow, Col: wrapCol}
			}
		}
	}

	// Column exceeds all segments — clamp to end of last segment
	return coords.WrapPoint{Row: lastSegRow, Col: lastSegLen}
}

func (w WrapSnapshot) WrapToSyntax(wp coords.WrapPoint) coords.SyntaxPoint {
	if wp.Row < 0 || wp.Row >= len(w.rowToSegment) {
		return coords.SyntaxPoint{Line: 0, Col: 0}
	}
	seg := w.Segments[w.rowToSegment[wp.Row]]

	// Clamp column to segment content length
	segLen := 0
	for _, span := range seg.Spans {
		segLen += len(span.Text)
	}
	col := wp.Col
	if col > segLen {
		col = segLen
	}
	if col < 0 {
		col = 0
	}

	return coords.SyntaxPoint{Line: seg.ModelLine, Col: seg.StartCol + col}
}

// SegmentLen returns the byte length of the content in a given display row.
func (w WrapSnapshot) SegmentLen(row int) int {
	if row < 0 || row >= len(w.rowToSegment) {
		return 0
	}
	seg := w.Segments[w.rowToSegment[row]]
	n := 0
	for _, span := range seg.Spans {
		n += len(span.Text)
	}
	return n
}

// VisualCol returns the visual column width (cell count) for a byte column
// within a given display row. Accounts for double-width CJK and tabs.
func (w WrapSnapshot) VisualCol(row, byteCol int) int {
	if row < 0 || row >= len(w.rowToSegment) {
		return 0
	}
	seg := w.Segments[w.rowToSegment[row]]
	text := w.segmentText(seg)
	visual := 0
	bytes := 0
	for bytes < len(text) && bytes < byteCol {
		r, size := utf8.DecodeRuneInString(text[bytes:])
		visual += runeWidthWithTab(r, visual)
		bytes += size
	}
	return visual
}

// ByteColFromVisual returns the byte column within a display row that
// corresponds to a target visual column width. If the visual column exceeds
// the row content, returns the segment's byte length (end of row).
func (w WrapSnapshot) ByteColFromVisual(row, visualCol int) int {
	if row < 0 || row >= len(w.rowToSegment) {
		return 0
	}
	seg := w.Segments[w.rowToSegment[row]]
	text := w.segmentText(seg)
	visual := 0
	bytes := 0
	for bytes < len(text) {
		r, size := utf8.DecodeRuneInString(text[bytes:])
		rw := runeWidthWithTab(r, visual)
		if visual+rw > visualCol {
			break
		}
		visual += rw
		bytes += size
	}
	return bytes
}

func (w WrapSnapshot) segmentText(seg WrapSegment) string {
	if len(seg.Spans) == 1 {
		return seg.Spans[0].Text
	}
	var b []byte
	for _, span := range seg.Spans {
		b = append(b, span.Text...)
	}
	return string(b)
}

func (w WrapSnapshot) ModelLineToFirstRow(line int) int {
	if line < 0 || line >= len(w.lineToFirstRow) {
		return 0
	}
	return w.lineToFirstRow[line]
}

func (w WrapSnapshot) RowToModelLine(row int) int {
	if row < 0 || row >= len(w.rowToSegment) {
		return 0
	}
	return w.Segments[w.rowToSegment[row]].ModelLine
}

type WrapMap struct {
	width int
}

func NewWrapMap(width int) WrapMap {
	return WrapMap{width: width}
}

func (w WrapMap) SetWidth(width int) WrapMap {
	w.width = width
	return w
}

func runeWidthWithTab(r rune, currentWidth int) int {
	if r == '\t' {
		return 4 - (currentWidth % 4)
	}
	return runewidth.RuneWidth(r)
}

func (w WrapMap) Sync(ss SyntaxSnapshot) WrapSnapshot {
	var segments []WrapSegment
	var rowToSegment []int
	lineToFirstRow := make([]int, len(ss.Lines))

	for lineIdx, line := range ss.Lines {
		lineToFirstRow[lineIdx] = len(segments)

		if len(line.Spans) == 0 {
			segments = append(segments, WrapSegment{
				Spans:     nil,
				ModelLine: lineIdx,
				WrapIndex: 0,
				StartCol:  0,
			})
			rowToSegment = append(rowToSegment, len(segments)-1)
			continue
		}

		if w.width <= 0 {
			segments = append(segments, WrapSegment{
				Spans:     line.Spans,
				ModelLine: lineIdx,
				WrapIndex: 0,
				StartCol:  0,
			})
			rowToSegment = append(rowToSegment, len(segments)-1)
			continue
		}

		// Build a concatenated text for wrap-break calculations and a mapping
		// from byte offset in concatenated text back to the originating span.
		var spanRefs []spanRef
		var textBuf []byte
		for i, s := range line.Spans {
			spanRefs = append(spanRefs, spanRef{index: i, startOff: len(textBuf)})
			textBuf = append(textBuf, s.Text...)
		}
		text := string(textBuf)

		if len(text) == 0 {
			segments = append(segments, WrapSegment{
				Spans:     line.Spans,
				ModelLine: lineIdx,
				WrapIndex: 0,
				StartCol:  0,
			})
			rowToSegment = append(rowToSegment, len(segments)-1)
			continue
		}

		startCol := 0
		wrapIndex := 0

		for startCol < len(text) {
			remain := text[startCol:]

			currW := 0
			byteLen := 0
			lastSpaceBytes := -1

			i := 0
			for i < len(remain) {
				r, size := utf8.DecodeRuneInString(remain[i:])
				rw := runeWidthWithTab(r, currW)
				if currW+rw > w.width && byteLen > 0 {
					break
				}
				if r == ' ' || r == '\t' {
					lastSpaceBytes = byteLen + size
				}
				currW += rw
				byteLen += size
				i += size
			}

			if byteLen == 0 && len(remain) > 0 {
				_, size := utf8.DecodeRuneInString(remain)
				byteLen = size
			} else if byteLen < len(remain) && lastSpaceBytes > 0 {
				byteLen = lastSpaceBytes
			}

			// Slice original spans to produce correct BufferStart/BufferEnd
			segStart := startCol
			segEnd := startCol + byteLen
			segSpans := sliceOriginalSpans(line.Spans, spanRefs, segStart, segEnd)

			segments = append(segments, WrapSegment{
				Spans:     segSpans,
				ModelLine: lineIdx,
				WrapIndex: wrapIndex,
				StartCol:  startCol,
			})
			rowToSegment = append(rowToSegment, len(segments)-1)

			startCol += byteLen
			wrapIndex++
		}
	}

	return WrapSnapshot{
		Segments:       segments,
		TotalRows:      len(segments),
		rowToSegment:   rowToSegment,
		lineToFirstRow: lineToFirstRow,
	}
}

// spanRef maps a span's position within the concatenated text string.
type spanRef struct {
	index    int // index into the original line.Spans slice
	startOff int // byte offset where this span starts in the concatenated text
}

// sliceOriginalSpans extracts the sub-spans from the original SyntaxSpan slice
// that cover the byte range [segStart, segEnd) in the concatenated text.
// Each output span retains correct BufferStart/BufferEnd from the originals.
func sliceOriginalSpans(spans []SyntaxSpan, refs []spanRef, segStart, segEnd int) []SyntaxSpan {
	var result []SyntaxSpan

	for _, ref := range refs {
		s := spans[ref.index]
		spanStart := ref.startOff
		spanEnd := spanStart + len(s.Text)

		// Skip spans entirely before or after the segment range.
		if spanEnd <= segStart || spanStart >= segEnd {
			continue
		}

		// Compute the slice of this span's text that falls within [segStart, segEnd).
		localStart := 0
		if segStart > spanStart {
			localStart = segStart - spanStart
		}
		localEnd := len(s.Text)
		if segEnd < spanEnd {
			localEnd = segEnd - spanStart
		}

		// Compute BufferStart/BufferEnd for the sliced portion.
		// The original span's BufferStart corresponds to localStart=0 in s.Text.
		// We need to map from text-space offset to buffer-space offset.
		//
		// For Revealed spans: text bytes == buffer bytes, so offset arithmetic is direct.
		// For Rendered spans: the text is shorter than the buffer range (hidden delims).
		//   In this case we CANNOT split — the entire rendered span maps to the full
		//   buffer range. We include it whole if any part intersects the segment.
		if s.State == Rendered {
			// Rendered spans cannot be sub-sliced because their text doesn't
			// correspond byte-for-byte to the buffer range.
			// Include the full span text that overlaps the segment.
			out := s
			out.Text = s.Text[localStart:localEnd]
			// CellMap is only valid when the full text is preserved; nil it
			// for partial slices since byte offsets no longer correspond.
			if localStart > 0 || localEnd < len(s.Text) {
				out.CellMap = nil
			}
			result = append(result, out)
		} else {
			// Revealed spans: text == buffer content, direct offset mapping.
			out := s
			out.Text = s.Text[localStart:localEnd]
			out.BufferStart = s.BufferStart + localStart
			out.BufferEnd = s.BufferStart + localEnd
			// Slice CellMap to match the sliced text portion.
			if s.CellMap != nil && localStart == 0 && localEnd == len(s.Text) {
				// Full span — keep CellMap as-is.
			} else {
				out.CellMap = nil
			}
			result = append(result, out)
		}
	}

	return result
}
