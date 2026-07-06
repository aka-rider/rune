package display_test

import (
	"strings"
	"testing"

	"rune/pkg/editor/buffer"
	"rune/pkg/editor/cursor"
	"rune/pkg/editor/display"
)

// foldedLine renders a single model line in folded (non-revealed) mode and
// returns the concatenation of its display span texts — i.e. what the user sees.
func foldedLine(t *testing.T, content string, line int) string {
	t.Helper()
	buf := buffer.New(content)
	sm := display.NewSyntaxMap()
	_, syn := sm.SyncNoReveal(buf, cursor.NewCursorSet(0))
	wrap := display.NewWrapMap(0).Sync(syn)
	ds := display.BuildSnapshot(wrap)
	if line >= len(ds.Lines) {
		t.Fatalf("line %d out of range (%d lines)", line, len(ds.Lines))
	}
	var b strings.Builder
	for _, sp := range ds.Lines[line].Spans {
		b.WriteString(sp.Text)
	}
	return b.String()
}

func foldedSpans(t *testing.T, content string, line int) []display.SyntaxSpan {
	t.Helper()
	buf := buffer.New(content)
	sm := display.NewSyntaxMap()
	_, syn := sm.SyncNoReveal(buf, cursor.NewCursorSet(0))
	if line >= len(syn.Lines) {
		t.Fatalf("line %d out of range", line)
	}
	return syn.Lines[line].Spans
}

// BUG1: a wiki link with an alias must render as just the alias, not a garbled
// mix of the target bytes and the alias.
func TestInlineEmit_WikiLinkAlias(t *testing.T) {
	// The exact string from the bug report.
	const content = "[[Authentication (sorry, no whitepaper for you).md|Original]]"
	got := foldedLine(t, content, 0)
	if got != "Original" {
		t.Errorf("wiki alias render: got %q, want %q", got, "Original")
	}

	// The navigable span must carry the resolved target for navigation.
	var found bool
	for _, sp := range foldedSpans(t, content, 0) {
		if sp.Kind == display.TokenWikiLink {
			found = true
			if sp.Text != "Original" {
				t.Errorf("wiki span text: got %q, want %q", sp.Text, "Original")
			}
			if sp.WikiLinkTarget != "Authentication (sorry, no whitepaper for you).md" {
				t.Errorf("wiki target: got %q", sp.WikiLinkTarget)
			}
		}
	}
	if !found {
		t.Fatal("no wiki-link span")
	}
}

// A wiki link without an alias renders as the target.
func TestInlineEmit_WikiLinkNoAlias(t *testing.T) {
	if got := foldedLine(t, "[[page]]", 0); got != "page" {
		t.Errorf("got %q, want %q", got, "page")
	}
}

// A wiki link not at column 0 must still fold correctly (line-relative offsets).
func TestInlineEmit_WikiLinkMidLine(t *testing.T) {
	if got := foldedLine(t, "see [[page|here]] now", 0); got != "see here now" {
		t.Errorf("got %q, want %q", got, "see here now")
	}
}

// BUG3: bold wrapping a link must render as the bold link label (no literal **),
// as a single span carrying both the link role and the bold mark.
func TestInlineEmit_BoldLink(t *testing.T) {
	if got := foldedLine(t, "**[Ghostty](https://ghostty.org/)**", 0); got != "Ghostty" {
		t.Errorf("bold link render: got %q, want %q", got, "Ghostty")
	}

	var linkSpan *display.SyntaxSpan
	spans := foldedSpans(t, "**[Ghostty](https://ghostty.org/)**", 0)
	for i := range spans {
		if spans[i].Kind == display.TokenLink {
			linkSpan = &spans[i]
		}
	}
	if linkSpan == nil {
		t.Fatal("no link span")
	}
	if !linkSpan.Marks.Has(display.MarkBold) {
		t.Errorf("link span should carry MarkBold, marks=%d", linkSpan.Marks)
	}
	if linkSpan.Text != "Ghostty" {
		t.Errorf("link text: got %q", linkSpan.Text)
	}
}

// Decorations compose across mixed content: **a [x](y) b** → bold "a ", bold link
// "x", bold " b" — three contiguous spans, no leaked delimiters.
func TestInlineEmit_BoldMixedContent(t *testing.T) {
	if got := foldedLine(t, "**a [x](y) b**", 0); got != "a x b" {
		t.Errorf("got %q, want %q", got, "a x b")
	}
	var link, text int
	for _, sp := range foldedSpans(t, "**a [x](y) b**", 0) {
		if !sp.Marks.Has(display.MarkBold) {
			continue
		}
		switch sp.Kind {
		case display.TokenLink:
			link++
		case display.TokenText:
			text++
		}
	}
	if link != 1 {
		t.Errorf("expected 1 bold link span, got %d", link)
	}
	if text != 2 {
		t.Errorf("expected 2 bold text spans, got %d", text)
	}
}

// BUG4: an empty-alt image must render its URL as the label without duplication
// or scramble.
func TestInlineEmit_EmptyAltImage(t *testing.T) {
	if got := foldedLine(t, "![](assets/rune-intro.gif)", 0); got != "assets/rune-intro.gif" {
		t.Errorf("empty-alt image render: got %q, want %q", got, "assets/rune-intro.gif")
	}
	// Must be classified as an image (not a navigable link), so it routes through
	// image-row rendering rather than link styling/navigation.
	var found bool
	for _, sp := range foldedSpans(t, "![](assets/rune-intro.gif)", 0) {
		if sp.Kind == display.TokenImage {
			found = true
			if sp.LinkRole() != display.LinkRoleImage {
				t.Errorf("empty-alt image LinkRole = %v, want Image", sp.LinkRole())
			}
		}
	}
	if !found {
		t.Fatal("empty-alt image produced no TokenImage span")
	}
}

// An image with alt text renders the alt text.
func TestInlineEmit_AltImage(t *testing.T) {
	if got := foldedLine(t, "![logo](assets/logo.png)", 0); got != "logo" {
		t.Errorf("got %q, want %q", got, "logo")
	}
}

// A plain inline link renders as its label.
func TestInlineEmit_PlainLink(t *testing.T) {
	if got := foldedLine(t, "[Ghostty](https://ghostty.org/)", 0); got != "Ghostty" {
		t.Errorf("got %q, want %q", got, "Ghostty")
	}
}

// Triple-emphasis composes both marks onto one span.
func TestInlineEmit_BoldItalic(t *testing.T) {
	if got := foldedLine(t, "***x***", 0); got != "x" {
		t.Errorf("got %q, want %q", got, "x")
	}
	var found bool
	for _, sp := range foldedSpans(t, "***x***", 0) {
		if sp.Marks.Has(display.MarkBold) && sp.Marks.Has(display.MarkItalic) {
			found = true
		}
	}
	if !found {
		t.Error("***x*** should produce a span with both bold and italic marks")
	}
}

// Underscore emphasis is recognized.
func TestInlineEmit_UnderscoreItalic(t *testing.T) {
	if got := foldedLine(t, "_italic_", 0); got != "italic" {
		t.Errorf("got %q, want %q", got, "italic")
	}
}

// Bold INSIDE a link: the link folds to its label and carries the bold mark.
func TestInlineEmit_BoldInsideLink(t *testing.T) {
	if got := foldedLine(t, "[**b**](u)", 0); got != "b" {
		t.Errorf("got %q, want %q", got, "b")
	}
	for _, sp := range foldedSpans(t, "[**b**](u)", 0) {
		if sp.Kind == display.TokenLink && !sp.Marks.Has(display.MarkBold) {
			t.Error("link-with-bold-text span should carry MarkBold")
		}
	}
}

// A code span inside a link keeps its code role (documented limitation: it is
// rendered as code and is not separately navigable) — it must NOT leak delimiters.
func TestInlineEmit_CodeInsideLink(t *testing.T) {
	if got := foldedLine(t, "[`c`](u)", 0); got != "c" {
		t.Errorf("code-in-link render: got %q, want %q (no leaked delimiters)", got, "c")
	}
}

// An empty-text link uses the URL as its label (and stays navigable).
func TestInlineEmit_EmptyTextLink(t *testing.T) {
	if got := foldedLine(t, "[](https://x.com)", 0); got != "https://x.com" {
		t.Errorf("got %q, want %q", got, "https://x.com")
	}
}

// Reference and shortcut links fold to their label.
func TestInlineEmit_ReferenceLink(t *testing.T) {
	content := "[ref][id]\n\n[id]: https://example.com"
	if got := foldedLine(t, content, 0); got != "ref" {
		t.Errorf("reference link: got %q, want %q", got, "ref")
	}
}

// Every folded navigable-link span's visible runes must come from the buffer
// bytes its CellMap points at (the deep fidelity property the fuzz invariant
// also checks).
func TestInlineEmit_LinkCellMapFidelity(t *testing.T) {
	for _, content := range []string{
		"[[Authentication (sorry).md|Original]]",
		"**[Ghostty](https://ghostty.org/)**",
		"see [[page|here]] now",
		"[plain](u) and **[bold](v)** links",
	} {
		buf := buffer.New(content)
		sm := display.NewSyntaxMap()
		_, syn := sm.SyncNoReveal(buf, cursor.NewCursorSet(0))
		for _, sp := range syn.Lines[0].Spans {
			if sp.State != display.Rendered || sp.CellMap == nil {
				continue
			}
			if sp.LinkRole() != display.LinkRoleNavigable {
				continue
			}
			ri := 0
			for _, r := range sp.Text {
				off := sp.CellMap[ri].BufOffset
				ri++
				if off < 0 || off >= len(content) {
					t.Fatalf("%q: cellmap offset %d out of range", content, off)
				}
				if rune(content[off]) != r && content[off] != byte(r) {
					// compare first byte for ASCII labels (sufficient here)
					t.Errorf("%q: rune %q maps to buffer byte %q", content, string(r), string(content[off]))
				}
			}
		}
	}
}

// TestInlineEmit_UnmatchedDelimiterInLinkLabel pins BUG5: a link label with an
// unpaired emphasis delimiter (no closing "*" anywhere in the paragraph) used
// to split into two sibling spans, and only the first absorbed the link's
// opening "[" into its hidden prefix — the second span's hidden prefix was
// empty. CellMapFidelity above wouldn't catch this: each span's CellMap was
// already internally self-consistent: the bug is BufferStart disagreeing with
// CellMap[0].BufOffset, which this test checks directly.
func TestInlineEmit_UnmatchedDelimiterInLinkLabel(t *testing.T) {
	const content = "[*00000](b.md)"
	for _, sp := range foldedSpans(t, content, 0) {
		if sp.State != display.Rendered || sp.CellMap == nil || sp.LinkRole() != display.LinkRoleNavigable {
			continue
		}
		prefix := content[sp.BufferStart:sp.CellMap[0].BufOffset]
		if !strings.HasPrefix(prefix, "[") {
			t.Errorf("span %q: hidden prefix %q does not start with \"[\"", sp.Text, prefix)
		}
	}
}
