package editor

import (
	"go/parser"
	"go/token"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"rune/pkg/editor/cursor"
	"rune/pkg/editor/display"
)

// ==========================================================================
// Gate 1: Rendered Go code fence has highlighted token spans
// ==========================================================================

func TestCodeFenceHighlight_GoKeywordStringComment(t *testing.T) {
	// Use the ChromaHighlighter to highlight a Go snippet
	h := ChromaHighlighter()
	source := `package main

import "fmt"

// Hello prints a greeting
func main() {
	fmt.Println("hello")
}`

	spans, err := h("go", source)
	if err != nil {
		t.Fatalf("highlight error: %v", err)
	}
	if len(spans) == 0 {
		t.Fatal("expected non-empty spans")
	}

	// Check that we have keyword, string, and comment classes
	classes := make(map[string]bool)
	for _, sp := range spans {
		classes[sp.Class] = true
	}

	for _, want := range []string{"keyword", "string", "comment"} {
		if !classes[want] {
			t.Errorf("expected class %q in spans, got classes: %v", want, classKeys(classes))
		}
	}
}

func TestCodeFenceHighlight_LanguageLabelRendered(t *testing.T) {
	// Set up a full editor with a markdown file containing a Go code fence
	md := "# Title\n```go\npackage main\n```\n"
	m := newTestEditor(md)
	m = m.syncDisplay()

	// Debug: directly test renderSpan on the opening fence marker span
	for i, line := range m.snapshot.Lines {
		for _, sp := range line.Spans {
			if sp.Kind == display.TokenCodeFence && sp.Text == "" && sp.Language == "go" {
				rendered := m.renderSpan(sp)
				t.Logf("line %d: renderSpan for fence marker returned %q", i, rendered)
				if !strings.Contains(rendered, "go") {
					t.Errorf("renderSpan for fence marker with lang='go' should contain 'go', got %q", rendered)
				}
			}
		}
	}

	// The View should contain the language label.
	view := m.View()
	if !strings.Contains(view, "go") {
		t.Errorf("expected language label 'go' in rendered view")
	}
}

// ==========================================================================
// Gate 2: Unknown language falls back without error
// ==========================================================================

func TestCodeFenceHighlight_UnknownLanguageFallback(t *testing.T) {
	h := ChromaHighlighter()
	spans, err := h("nonexistent_lang_xyz", "some code here")
	if err != nil {
		t.Fatalf("expected no error for unknown language, got: %v", err)
	}
	if spans != nil {
		t.Fatalf("expected nil spans for unknown language, got %d spans", len(spans))
	}
}

func TestCodeFenceHighlight_UnknownLanguageRendersFallback(t *testing.T) {
	// Full editor rendering with unknown language should not panic or error
	md := "# Title\n```unknownlang\nfoo bar baz\n```\n"
	m := newTestEditor(md)
	m = m.syncDisplay()

	// Should render without panic — content appears as plain text
	view := m.View()
	if !strings.Contains(view, "foo bar baz") {
		t.Errorf("expected code content in view for unknown language, got:\n%s", view)
	}
}

// ==========================================================================
// Gate 3: Revealed fence shows raw delimiters without highlighting
// ==========================================================================

func TestCodeFenceHighlight_RevealedShowsRawDelimiters(t *testing.T) {
	// When cursor is inside the fence, the display pipeline reveals the raw
	// source — fence markers should be visible in the snapshot and not go
	// through the highlighting path.
	md := "before\n```go\npackage main\n```\nafter"
	m := newTestEditor(md)
	m = m.syncDisplay() // initial sync with cursor at 0 (outside fence)

	// Move cursor into the fence body to trigger reveal
	cursorPos := len("before\n```go\n") // offset into "package main"
	m.cursors = cursor.NewCursorSet(cursorPos)
	m = m.syncDisplay()

	// Now the snapshot should show fence lines in Revealed state
	found := false
	for _, line := range m.snapshot.Lines {
		for _, sp := range line.Spans {
			if sp.Kind == display.TokenCodeFence && sp.State == display.Revealed {
				found = true
				// Revealed spans show the raw text (fence markers or code)
				// The fence opening line text should contain ```
				if strings.Contains(sp.Text, "```") {
					// Good - raw delimiter is visible
				}
			}
		}
	}
	if !found {
		t.Error("expected Revealed code fence spans when cursor is inside fence")
	}

	// Verify that raw delimiters are in the view
	view := m.View()
	if !strings.Contains(view, "```") {
		t.Errorf("expected raw fence delimiters in revealed view, got:\n%s", view)
	}
}

// ==========================================================================
// Gate 4: Domain display package remains free of UI/highlighter imports
// ==========================================================================

func TestCodeFenceHighlight_DisplayPackageNoChromaLipgloss(t *testing.T) {
	// Parse all .go files in pkg/editor/display/ and verify none import
	// chroma or lipgloss packages.
	displayDir := filepath.Join(".", "..", "..", "..", "editor", "display")

	entries, err := os.ReadDir(displayDir)
	if err != nil {
		t.Fatalf("failed to read display dir: %v", err)
	}

	fset := token.NewFileSet()
	for _, entry := range entries {
		if entry.IsDir() || !strings.HasSuffix(entry.Name(), ".go") {
			continue
		}
		if strings.HasSuffix(entry.Name(), "_test.go") {
			continue // skip test files
		}

		path := filepath.Join(displayDir, entry.Name())
		f, err := parser.ParseFile(fset, path, nil, parser.ImportsOnly)
		if err != nil {
			t.Fatalf("failed to parse %s: %v", path, err)
		}

		for _, imp := range f.Imports {
			importPath := strings.Trim(imp.Path.Value, `"`)
			if strings.Contains(importPath, "chroma") {
				t.Errorf("%s imports chroma package %q — display must not depend on highlighter", entry.Name(), importPath)
			}
			if strings.Contains(importPath, "lipgloss") {
				t.Errorf("%s imports lipgloss package %q — display must not depend on UI styles", entry.Name(), importPath)
			}
		}
	}
}

// ==========================================================================
// Preflight fixture: consume actual TokenCodeFence spans from real markdown
// ==========================================================================

func TestGolden_CodeFence_SpanStructure(t *testing.T) {
	// Feed a real markdown document through the editor display pipeline and
	// verify the code fence spans have the expected structure.
	md := "# Hello\n\nSome text.\n\n```go\npackage main\n\nimport \"fmt\"\n\nfunc main() {\n\tfmt.Println(\"hello\")\n}\n```\n\nDone.\n"
	m := newTestEditor(md)
	m = m.syncDisplay()

	// Collect all CodeFence spans from the snapshot
	type fenceSpan struct {
		text     string
		state    display.RevealState
		language string
		blockID  int
	}

	var fenceSpans []fenceSpan
	for _, line := range m.snapshot.Lines {
		for _, sp := range line.Spans {
			if sp.Kind == display.TokenCodeFence {
				fenceSpans = append(fenceSpans, fenceSpan{
					text:     sp.Text,
					state:    sp.State,
					language: sp.Language,
					blockID:  sp.BlockID,
				})
			}
		}
	}

	if len(fenceSpans) == 0 {
		t.Fatal("no TokenCodeFence spans found in snapshot")
	}

	// All fence spans should share the same BlockID
	blockID := fenceSpans[0].blockID
	for i, fs := range fenceSpans {
		if fs.blockID != blockID {
			t.Errorf("span %d has different blockID %d vs %d", i, fs.blockID, blockID)
		}
	}

	// All should be Rendered (cursor not in fence)
	for i, fs := range fenceSpans {
		if fs.state != display.Rendered {
			t.Errorf("span %d should be Rendered (cursor outside fence), got Revealed", i)
		}
	}

	// Language should be "go" for all
	for i, fs := range fenceSpans {
		if fs.language != "go" {
			t.Errorf("span %d: expected language 'go', got %q", i, fs.language)
		}
	}

	// The first and last fence spans should have empty text (marker lines)
	if fenceSpans[0].text != "" {
		t.Errorf("opening fence marker span should have empty text, got %q", fenceSpans[0].text)
	}
	lastIdx := len(fenceSpans) - 1
	if fenceSpans[lastIdx].text != "" {
		t.Errorf("closing fence marker span should have empty text, got %q", fenceSpans[lastIdx].text)
	}

	// Body spans should contain actual code lines
	bodySpans := fenceSpans[1:lastIdx]
	if len(bodySpans) == 0 {
		t.Fatal("expected body spans between markers")
	}

	// First body span should be "package main"
	if bodySpans[0].text != "package main" {
		t.Errorf("first body span: expected 'package main', got %q", bodySpans[0].text)
	}
}

func TestGolden_CodeFence_HighlightedOutput(t *testing.T) {
	// Verify that a rendered code fence with Go code produces highlighted
	// output containing distinct styled regions
	md := "```go\nfunc main() {\n\tfmt.Println(\"hello\")\n}\n```\n"
	m := newTestEditor(md)
	m = m.syncDisplay()

	view := m.View()

	// The view should contain ANSI escape sequences (indicating styled output)
	if !strings.Contains(view, "\x1b[") {
		t.Error("expected ANSI escape sequences in highlighted code output")
	}

	// The rendered output should contain the code text
	if !strings.Contains(view, "func") {
		t.Errorf("expected 'func' in view output")
	}
	if !strings.Contains(view, "Println") {
		t.Errorf("expected 'Println' in view output")
	}
	if !strings.Contains(view, "hello") {
		t.Errorf("expected 'hello' in view output")
	}
}

// ==========================================================================
// Helpers
// ==========================================================================

func classKeys(m map[string]bool) []string {
	var keys []string
	for k := range m {
		keys = append(keys, k)
	}
	return keys
}
