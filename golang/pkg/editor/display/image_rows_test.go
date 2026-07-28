package display

import (
	"reflect"
	"testing"
)

// buildSnap constructs a one-row-per-line snapshot from the given lines,
// mirroring BuildSnapshot's invariants (TotalRows == len(Lines), and the
// private index arrays). modelLineCount is the number of distinct model lines.
func buildSnap(lines []DisplayLine, modelLineCount int) DisplaySnapshot {
	rowToModelLine := make([]int, len(lines))
	lineToFirstRow := make([]int, modelLineCount)
	seen := make([]bool, modelLineCount)
	for i, l := range lines {
		rowToModelLine[i] = l.ModelLine
		if l.ModelLine >= 0 && l.ModelLine < modelLineCount && !seen[l.ModelLine] {
			lineToFirstRow[l.ModelLine] = i
			seen[l.ModelLine] = true
		}
	}
	return DisplaySnapshot{
		Lines:          lines,
		TotalRows:      len(lines),
		rowToModelLine: rowToModelLine,
		lineToFirstRow: lineToFirstRow,
	}
}

func imageSpan(path string) DisplaySpan {
	return DisplaySpan{Kind: TokenImage, State: Rendered, Text: "alt", ImagePath: path}
}

func textLine(model int, text string) DisplayLine {
	return DisplayLine{Spans: []DisplaySpan{{Kind: TokenText, State: Revealed, Text: text}}, ModelLine: model}
}

func imageLine(model int, path string) DisplayLine {
	return DisplayLine{Spans: []DisplaySpan{imageSpan(path)}, ModelLine: model}
}

func dimsConst(cols, rows int) func(string) ImageDims {
	return func(string) ImageDims { return ImageDims{Cols: cols, Rows: rows} }
}

func TestExpandImageRows_GrowsRowCount(t *testing.T) {
	in := buildSnap([]DisplayLine{
		textLine(0, "before"),
		imageLine(1, "a.png"),
		textLine(2, "after"),
	}, 3)

	out := ExpandImageRows(in, dimsConst(8, 5))

	// 1 + 5 + 1 = 7 rows.
	if out.TotalRows != 7 {
		t.Fatalf("TotalRows=%d, want 7", out.TotalRows)
	}
	if len(out.Lines) != 7 {
		t.Fatalf("len(Lines)=%d, want 7", len(out.Lines))
	}
	anchor := out.Lines[1]
	if anchor.ImagePath != "a.png" || anchor.ImageRowIndex != 0 || anchor.ImageRowCount != 5 || anchor.ImageCols != 8 {
		t.Errorf("anchor metadata wrong: %+v", anchor)
	}
	if len(anchor.Spans) != 1 || anchor.Spans[0].Kind != TokenImage {
		t.Errorf("anchor must keep original image span, got %+v", anchor.Spans)
	}
	for r := 1; r < 5; r++ {
		cont := out.Lines[1+r]
		if cont.ImageRowIndex != r || cont.ImageRowCount != 5 || cont.ImagePath != "a.png" {
			t.Errorf("continuation row %d metadata wrong: %+v", r, cont)
		}
		if len(cont.Spans) != 0 {
			t.Errorf("continuation row %d should have empty spans, got %+v", r, cont.Spans)
		}
		if cont.ModelLine != 1 {
			t.Errorf("continuation row %d ModelLine=%d, want 1", r, cont.ModelLine)
		}
	}
}

func TestExpandImageRows_IndexIntegrity(t *testing.T) {
	in := buildSnap([]DisplayLine{
		textLine(0, "before"),
		imageLine(1, "a.png"),
		textLine(2, "after"),
	}, 3)

	out := ExpandImageRows(in, dimsConst(8, 5))

	if out.TotalRows != len(out.Lines) {
		t.Fatalf("TotalRows %d != len(Lines) %d", out.TotalRows, len(out.Lines))
	}
	// Every row's RowToModelLine matches its source line.
	for r := 0; r < out.TotalRows; r++ {
		if out.RowToModelLine(r) != out.Lines[r].ModelLine {
			t.Errorf("RowToModelLine(%d)=%d, want %d", r, out.RowToModelLine(r), out.Lines[r].ModelLine)
		}
	}
	// ModelLineToFirstRow is the first row of each model line.
	wantFirst := map[int]int{0: 0, 1: 1, 2: 6}
	for ml, want := range wantFirst {
		if got := out.ModelLineToFirstRow(ml); got != want {
			t.Errorf("ModelLineToFirstRow(%d)=%d, want %d", ml, got, want)
		}
	}
}

func TestExpandImageRows_NoExpansionWhenRowsOne(t *testing.T) {
	in := buildSnap([]DisplayLine{
		textLine(0, "before"),
		imageLine(1, "a.png"),
		textLine(2, "after"),
	}, 3)

	out := ExpandImageRows(in, dimsConst(0, 1))

	if !reflect.DeepEqual(in, out) {
		t.Errorf("expected snapshot unchanged when all images report Rows=1")
	}
}

func TestExpandImageRows_TwoImagesIndependent(t *testing.T) {
	in := buildSnap([]DisplayLine{
		imageLine(0, "a.png"),
		textLine(1, "mid"),
		imageLine(2, "b.png"),
	}, 3)

	out := ExpandImageRows(in, func(p string) ImageDims {
		if p == "a.png" {
			return ImageDims{Cols: 4, Rows: 3}
		}
		return ImageDims{Cols: 6, Rows: 2}
	})

	// 3 + 1 + 2 = 6 rows.
	if out.TotalRows != 6 {
		t.Fatalf("TotalRows=%d, want 6", out.TotalRows)
	}
	if out.Lines[0].ImagePath != "a.png" || out.Lines[0].ImageRowCount != 3 {
		t.Errorf("first image anchor wrong: %+v", out.Lines[0])
	}
	if out.Lines[3].ModelLine != 1 || out.Lines[3].ImagePath != "" {
		t.Errorf("mid text row wrong: %+v", out.Lines[3])
	}
	if out.Lines[4].ImagePath != "b.png" || out.Lines[4].ImageRowCount != 2 {
		t.Errorf("second image anchor wrong: %+v", out.Lines[4])
	}
}

func TestExpandImageRows_RevealedImageNotExpanded(t *testing.T) {
	revealed := DisplayLine{
		Spans:     []DisplaySpan{{Kind: TokenImage, State: Revealed, Text: "![alt](a.png)", ImagePath: "a.png"}},
		ModelLine: 0,
	}
	in := buildSnap([]DisplayLine{revealed}, 1)

	out := ExpandImageRows(in, dimsConst(8, 5))
	if out.TotalRows != 1 {
		t.Errorf("revealed image must not expand, got TotalRows=%d", out.TotalRows)
	}
}

func TestExpandImageRows_ListItemImageExpands(t *testing.T) {
	line := DisplayLine{
		Spans: []DisplaySpan{
			{Kind: TokenListMarker, State: Revealed, Text: "- "},
			imageSpan("y.png"),
		},
		ModelLine: 0,
	}
	in := buildSnap([]DisplayLine{line}, 1)

	out := ExpandImageRows(in, dimsConst(8, 4))
	if out.TotalRows != 4 {
		t.Errorf("list-item image should expand to 4 rows, got %d", out.TotalRows)
	}
	if out.Lines[0].ImagePath != "y.png" {
		t.Errorf("anchor ImagePath=%q, want y.png", out.Lines[0].ImagePath)
	}
}

func TestExpandImageRows_WrapRowPropagation(t *testing.T) {
	lines := []DisplayLine{
		textLine(0, "before"),
		imageLine(1, "a.png"),
		textLine(2, "after"),
	}
	// Stamp distinct WrapRow values as BuildSnapshot would, pre-expansion.
	for i := range lines {
		lines[i].WrapRow = i
	}
	in := buildSnap(lines, 3)

	out := ExpandImageRows(in, dimsConst(8, 5))

	anchor := out.Lines[1]
	if anchor.WrapRow != 1 {
		t.Errorf("anchor WrapRow=%d, want 1 (unchanged from source line)", anchor.WrapRow)
	}
	for r := 1; r < 5; r++ {
		cont := out.Lines[1+r]
		if cont.WrapRow != anchor.WrapRow {
			t.Errorf("continuation row %d WrapRow=%d, want anchor's WrapRow=%d", r, cont.WrapRow, anchor.WrapRow)
		}
	}
	if out.Lines[0].WrapRow != 0 || out.Lines[6].WrapRow != 2 {
		t.Errorf("surrounding text rows' WrapRow must be untouched, got %d and %d", out.Lines[0].WrapRow, out.Lines[6].WrapRow)
	}
}

func TestExpandImageRows_InlineImageNotExpanded(t *testing.T) {
	line := DisplayLine{
		Spans: []DisplaySpan{
			{Kind: TokenText, State: Revealed, Text: "text "},
			imageSpan("x.png"),
			{Kind: TokenText, State: Revealed, Text: " more"},
		},
		ModelLine: 0,
	}
	in := buildSnap([]DisplayLine{line}, 1)

	out := ExpandImageRows(in, dimsConst(8, 5))
	if out.TotalRows != 1 {
		t.Errorf("truly-inline image must not expand, got TotalRows=%d", out.TotalRows)
	}
	if _, ok := isStandaloneImageLine(line); ok {
		t.Error("isStandaloneImageLine should be false for inline image with surrounding text")
	}
}
