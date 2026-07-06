package display

import (
	"errors"
	"strings"
	"testing"
)

func TestParseFrontmatterYAML_Valid(t *testing.T) {
	lines := []string{"---", "title: Hello", "tags: [a, b]", "---"}
	out, err := parseFrontmatterYAML(lines, 3)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if out == nil {
		t.Fatal("expected non-nil map")
	}
	if _, ok := out["title"]; !ok {
		t.Error("expected 'title' key")
	}
	if _, ok := out["tags"]; !ok {
		t.Error("expected 'tags' key")
	}
}

func TestParseFrontmatterYAML_Empty(t *testing.T) {
	lines := []string{"---", "---"}
	out, err := parseFrontmatterYAML(lines, 1)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if out != nil {
		t.Errorf("expected nil map for empty frontmatter, got %v", out)
	}
}

func TestParseFrontmatterYAML_UnclosedQuote(t *testing.T) {
	lines := []string{"---", `title: "unclosed`, "---"}
	_, err := parseFrontmatterYAML(lines, 2)
	if err == nil {
		t.Fatal("expected error for unclosed quote, got nil")
	}
}

func TestParseFrontmatterYAML_TabIndent(t *testing.T) {
	lines := []string{"---", "key:", "\tvalue", "---"}
	_, err := parseFrontmatterYAML(lines, 3)
	if err == nil {
		t.Fatal("expected error for tab-indented YAML, got nil")
	}
}

func TestFrontmatterRenderedSpans_CollapsedNoError(t *testing.T) {
	block := mdBlock{
		kind:      TokenFrontmatter,
		id:        1,
		startLine: 0,
		endLine:   3,
	}
	spans := frontmatterRenderedSpans(block, 0, "---", 0, FrontmatterCollapsed, nil)
	if len(spans) == 0 {
		t.Fatal("expected spans")
	}
	if spans[0].Text != "··· frontmatter ···" {
		t.Errorf("expected '··· frontmatter ···', got %q", spans[0].Text)
	}
}

func TestFrontmatterRenderedSpans_CollapsedWithError(t *testing.T) {
	block := mdBlock{
		kind:      TokenFrontmatter,
		id:        1,
		startLine: 0,
		endLine:   3,
	}
	spans := frontmatterRenderedSpans(block, 0, "---", 0, FrontmatterCollapsed, errors.New("some error"))
	if len(spans) == 0 {
		t.Fatal("expected spans")
	}
	if spans[0].Text != "··· frontmatter (invalid YAML) ···" {
		t.Errorf("expected error label, got %q", spans[0].Text)
	}
}

func TestFrontmatterRenderedSpans_SourceModeIgnoresError(t *testing.T) {
	block := mdBlock{
		kind:      TokenFrontmatter,
		id:        1,
		startLine: 0,
		endLine:   3,
	}
	lineText := "---"
	spans := frontmatterRenderedSpans(block, 0, lineText, 0, FrontmatterSource, errors.New("some error"))
	if len(spans) == 0 {
		t.Fatal("expected spans")
	}
	if spans[0].Text != lineText {
		t.Errorf("source mode: expected raw lineText %q, got %q", lineText, spans[0].Text)
	}
	if strings.Contains(spans[0].Text, "invalid YAML") {
		t.Error("source mode should not show error label")
	}
}
