package display_test

import (
	"go/parser"
	"go/token"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"rune/pkg/editor/buffer"
	"rune/pkg/editor/cursor"
	"rune/pkg/editor/display"
)

func TestMonotonicity(t *testing.T) {
	buf := buffer.New("word1 word2")
	cursors := cursor.NewCursorSet(0)
	sMap := display.NewSyntaxMap()
	_, sSnapshot := sMap.Sync(buf, cursors)

	for a := 0; a < buf.Len()-1; a++ {
		for b := a + 1; b < buf.Len(); b++ {
			bpA := buf.OffsetToLineCol(a)
			bpB := buf.OffsetToLineCol(b)
			if bpA.Line == bpB.Line {
				spA := sSnapshot.BufferToSyntax(bpA)
				spB := sSnapshot.BufferToSyntax(bpB)
				if spA.Col > spB.Col {
					t.Errorf("Monotonicity violated: offset %d (spA col %d) > offset %d (spB col %d)", a, spA.Col, b, spB.Col)
				}
			}
		}
	}
}

func TestWrapMap_WidthAndTabs(t *testing.T) {
	text := "\ta\tbc\t中ñ" // \t (0->4), a (4->5), \t(5->8), b(8->9), c(9->10), \t(10->12), 中(12->14), ñ(14->15 in cells, 2 bytes)
	buf := buffer.New(text)
	cursors := cursor.NewCursorSet(0)
	sMap := display.NewSyntaxMap()
	_, sSnapshot := sMap.Sync(buf, cursors)

	wMap := display.NewWrapMap(8)
	wSnapshot := wMap.Sync(sSnapshot)

	if len(wSnapshot.Segments) < 2 {
		t.Fatalf("Expected line to wrap")
	}
	seg0 := string(wSnapshot.Segments[0].Spans[0].Text)
	if seg0 != "\ta\t" {
		t.Errorf("Gate 4 failed: expected segment 0 to be '\\ta\\t' (8 cells), got %q", seg0)
	}

	text2 := "abcdef中ñ"
	buf2 := buffer.New(text2)
	_, sSnapshot2 := sMap.Sync(buf2, cursors)

	wMap2 := display.NewWrapMap(7)
	wSnapshot2 := wMap2.Sync(sSnapshot2)
	seg2_0 := wSnapshot2.Segments[0].Spans[0].Text
	if seg2_0 != "abcdef" {
		t.Errorf("Gate 5/6 failed: expected 'abcdef' at wrap 7, got %q", seg2_0)
	}
	seg2_1 := wSnapshot2.Segments[1].Spans[0].Text
	if seg2_1 != "中ñ" {
		t.Errorf("Gate 5/6 failed: expected '中ñ' for second segment, got %q", seg2_1)
	}
}

func TestSnapshotSlice(t *testing.T) {
	text := "1\n2\n3\n4\n5\n6\n7\n8\n9\n10"
	buf := buffer.New(text)
	cursors := cursor.NewCursorSet(0)
	sMap := display.NewSyntaxMap()
	_, sSnapshot := sMap.Sync(buf, cursors)
	wMap := display.NewWrapMap(80)
	wSnapshot := wMap.Sync(sSnapshot)
	dSnapshot := display.BuildSnapshot(wSnapshot)

	if dSnapshot.TotalRows != 10 {
		t.Fatalf("Expected 10 total rows, got %d", dSnapshot.TotalRows)
	}

	slices := dSnapshot.Slice(2, 5)
	if len(slices) != 5 {
		t.Errorf("Expected slice of height 5, got %d", len(slices))
	}

	slicesEnd := dSnapshot.Slice(8, 5)
	if len(slicesEnd) != 2 {
		t.Errorf("Expected slice of height 2, got %d", len(slicesEnd))
	}

	slicesPast := dSnapshot.Slice(15, 5)
	if len(slicesPast) != 0 {
		t.Errorf("Expected slice of height 0, got %d", len(slicesPast))
	}
}

// TestDisplayPackageNoBannedImports verifies that the pkg/editor/display/ domain
// package has no imports from lipgloss, chroma, or ultraviolet. The display
// package must emit semantic spans only — rendering is the UI layer's job.
func TestDisplayPackageNoBannedImports(t *testing.T) {
	banned := []string{"lipgloss", "chroma", "ultraviolet"}

	// Find the display package directory relative to this test file
	displayDir := "."
	entries, err := os.ReadDir(displayDir)
	if err != nil {
		t.Fatalf("cannot read display package dir: %v", err)
	}

	fset := token.NewFileSet()
	for _, entry := range entries {
		if entry.IsDir() || !strings.HasSuffix(entry.Name(), ".go") {
			continue
		}
		// Skip test files — they may import test helpers
		if strings.HasSuffix(entry.Name(), "_test.go") {
			continue
		}

		path := filepath.Join(displayDir, entry.Name())
		f, err := parser.ParseFile(fset, path, nil, parser.ImportsOnly)
		if err != nil {
			t.Fatalf("failed to parse %s: %v", entry.Name(), err)
		}

		for _, imp := range f.Imports {
			importPath := strings.Trim(imp.Path.Value, `"`)
			for _, b := range banned {
				if strings.Contains(importPath, b) {
					t.Errorf("file %s imports banned package %q (contains %q)",
						entry.Name(), importPath, b)
				}
			}
		}
	}
}
