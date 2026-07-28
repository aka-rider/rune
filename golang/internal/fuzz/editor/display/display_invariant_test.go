package display

import (
	"testing"

	"rune/internal/fuzz/snapshot"
	"rune/pkg/editor/buffer"
	"rune/pkg/editor/cursor"
	disp "rune/pkg/editor/display"
)

// buildSnap runs a markdown string through the real display pipeline in folded
// (non-revealed) mode and packages it as a fuzz Snapshot for invariant checking.
func buildSnap(content string, width int) snapshot.Snapshot {
	buf := buffer.New(content)
	sm := disp.NewSyntaxMap().SetWidth(width)
	_, syn := sm.SyncNoReveal(buf, cursor.NewCursorSet(0))
	wrap := disp.NewWrapMap(width).Sync(syn)
	return snapshot.Snapshot{
		Content: content,
		Syntax:  syn,
		Wrap:    wrap,
		Display: disp.BuildSnapshot(wrap),
	}
}

// No false positives: links/images that render correctly must pass Check,
// including the (now-fixed) former bug inputs. Tested unwrapped and wrapped.
func TestLinkInvariants_NoFalsePositives(t *testing.T) {
	cases := []string{
		"[Ghostty](https://ghostty.org/)",
		"[[label]]",
		"[[a (b).md|c]]",                      // BUG1 — fixed
		"**[Ghostty](https://ghostty.org/)**", // BUG3 — fixed
		"**a [x](y) b**",
		"![alt](img.png)",
		"![](assets/rune-intro.gif)", // BUG4 — fixed
		"![[image.png]]",
		"see [[page|here]] now",
		"plain text with no links",
		"a *italic* and **bold** and ~~strike~~",
		"[ref][id]\n\n[id]: https://example.com",
		"[](https://x.com)",       // empty-text link (URL as label)
		"[`code`](https://x.com)", // code inside link
		"***both***",              // bold+italic
		"a [x](y)* not emphasis",  // lone trailing * must not trip LINK-CLEAN
		"[*00000](b.md)",          // BUG5 (unmatched delim in link label) — fixed
	}
	for _, c := range cases {
		for _, w := range []int{0, 8} {
			if v := Check(buildSnap(c, w)); v != nil {
				t.Errorf("Check(%q, w=%d) = %s: %s; want nil", c, w, v.InvariantID, v.Message)
			}
		}
	}
}

// TestLinkInvariants_UnmatchedDelimiterInLinkLabel pins BUG5: found by
// FuzzHumanSession. A link label containing an unpaired emphasis delimiter
// (no closing "*" anywhere in the containing paragraph) causes goldmark to
// hand back the label as TWO sibling text nodes instead of one contiguous
// run — linkSpans only extends the delimiter width on the first, leaving the
// second with an empty hidden prefix.
func TestLinkInvariants_UnmatchedDelimiterInLinkLabel(t *testing.T) {
	cases := []string{
		"[*00000](b.md)",
		"# File A\n\n[*00000](b.md)\n[notes](notes/c.md)\n[x](missing.md)\n" +
			"[web](https://example.com)\n[mail](mailto:a@b.com)\n",
	}
	for _, c := range cases {
		for _, w := range []int{0, 8} {
			if v := Check(buildSnap(c, w)); v != nil {
				t.Errorf("Check(%q, w=%d) = %s: %s; want nil", c, w, v.InvariantID, v.Message)
			}
		}
	}
}

// TestTableInvariants_DecorativeSpansAbutNeighbors pins BUG7: a table row
// mixing decorative content (borders/padding/unstyled cells) with a styled
// span used to fail SPAN-COVER — the styled span got its own correct bounds
// (BUG6) but decorative spans on either side still claimed the whole row,
// so they no longer tiled. Covers both the originally-found shape (a link in
// a non-first cell) and the broader class (a styled span in the first cell).
func TestTableInvariants_DecorativeSpansAbutNeighbors(t *testing.T) {
	cases := []string{
		"| A | B |\n| --- | --- |\n| x | [bar](baz.md) |",
		"| **bold** | x |\n| --- | --- |\n| **bold** | x |",
	}
	for _, c := range cases {
		for _, w := range []int{0, 8} {
			if v := Check(buildSnap(c, w)); v != nil {
				t.Errorf("Check(%q, w=%d) = %s: %s; want nil", c, w, v.InvariantID, v.Message)
			}
		}
	}
}

// No false negatives: the invariants must FIRE on the structural corruption each
// bug produced, proving they are not dead checks. The snapshots are hand-crafted
// to reproduce the pre-fix span layout (the pipeline no longer produces it).
func TestLinkInvariants_DetectCorruption(t *testing.T) {
	wrapFromSyntax := func(lines []disp.SyntaxLine) disp.WrapSnapshot {
		var segs []disp.WrapSegment
		for i, l := range lines {
			segs = append(segs, disp.WrapSegment{Spans: l.Spans, ModelLine: i})
		}
		return disp.WrapSnapshot{Segments: segs, TotalRows: len(segs)}
	}
	mk := func(content string, spans []disp.SyntaxSpan) snapshot.Snapshot {
		lines := []disp.SyntaxLine{{Spans: spans}}
		return snapshot.Snapshot{
			Content: content,
			Syntax:  disp.SyntaxSnapshot{Lines: lines},
			Wrap:    wrapFromSyntax(lines),
		}
	}

	// BUG1: a wiki link whose span start landed inside the target, leaking the
	// "[[a (b).md|" prefix as plain text. The folded wiki span's hidden prefix is
	// "d|" — not a valid "[[" opener.
	bug1 := mk("[[a (b).md|c]]", []disp.SyntaxSpan{
		{Text: "[[a (b).m", Kind: disp.TokenText, State: disp.Revealed, BufferStart: 0, BufferEnd: 9},
		{Text: "c", Kind: disp.TokenWikiLink, State: disp.Rendered, BufferStart: 9, BufferEnd: 14,
			CellMap: []disp.CellMapping{{BufOffset: 11}}, WikiLinkTarget: "a (b).md"},
	})
	if v := Check(bug1); v == nil || v.InvariantID != "LINK-FOLD" {
		t.Errorf("BUG1 snapshot: got %v, want LINK-FOLD", v)
	}

	// BUG3: bold wrapping a link rendered as **Ghostty** — the "**" leaked as
	// Revealed siblings flanking the folded link.
	cm := make([]disp.CellMapping, 0, 7)
	for i := 3; i <= 9; i++ { // "Ghostty" lives at bytes 3..9 of "**[Ghostty](url)**"
		cm = append(cm, disp.CellMapping{BufOffset: i})
	}
	bug3 := mk("**[Ghostty](url)**", []disp.SyntaxSpan{
		{Text: "**", Kind: disp.TokenText, State: disp.Revealed, BufferStart: 0, BufferEnd: 2},
		{Text: "Ghostty", Kind: disp.TokenLink, State: disp.Rendered, BufferStart: 2, BufferEnd: 16,
			CellMap: cm, LinkURL: "url"},
		{Text: "**", Kind: disp.TokenText, State: disp.Revealed, BufferStart: 16, BufferEnd: 18},
	})
	if v := Check(bug3); v == nil || v.InvariantID != "LINK-CLEAN" {
		t.Errorf("BUG3 snapshot: got %v, want LINK-CLEAN", v)
	}

	// BUG5: an unmatched emphasis delimiter as the link's first child (no
	// closing "*" anywhere in the paragraph) split the label into two sibling
	// spans; linkSpans only extended the first's delimLeft, leaving "00000"
	// with delimLeft=0 — a zero-width hidden prefix.
	bug5 := mk("[*00000](b.md)", []disp.SyntaxSpan{
		{Text: "*", Kind: disp.TokenLink, State: disp.Rendered, BufferStart: 0, BufferEnd: 2,
			CellMap: []disp.CellMapping{{BufOffset: 1}}, LinkURL: "b.md"},
		{Text: "00000", Kind: disp.TokenLink, State: disp.Rendered, BufferStart: 2, BufferEnd: 14,
			CellMap: []disp.CellMapping{{BufOffset: 2}, {BufOffset: 3}, {BufOffset: 4}, {BufOffset: 5}, {BufOffset: 6}},
			LinkURL: "b.md"},
	})
	if v := Check(bug5); v == nil || v.InvariantID != "LINK-FOLD" {
		t.Errorf("BUG5 snapshot: got %v, want LINK-FOLD", v)
	}

	// BUG6: buildTableStyledSpans pinned every emitted span's BufferStart/
	// BufferEnd to the whole table row instead of the token's own bounds, so a
	// folded link in a non-first cell computed its hidden prefix against
	// everything before it in the row, not just its own "[".
	row := "| x | [bar](baz.md) |"
	bug6 := mk(row, []disp.SyntaxSpan{
		{Text: "bar", Kind: disp.TokenLink, State: disp.Rendered,
			BufferStart: 0, BufferEnd: len(row), // BUG: pinned to the whole row
			CellMap: []disp.CellMapping{{BufOffset: 7}, {BufOffset: 8}, {BufOffset: 9}},
			LinkURL: "baz.md"},
	})
	if v := Check(bug6); v == nil || v.InvariantID != "LINK-FOLD" {
		t.Errorf("BUG6 snapshot: got %v, want LINK-FOLD", v)
	}
}
