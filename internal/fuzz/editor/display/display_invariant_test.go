//go:build fuzzing

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
		"[[a (b).md|c]]",                       // BUG1 — fixed
		"**[Ghostty](https://ghostty.org/)**",  // BUG3 — fixed
		"**a [x](y) b**",
		"![alt](img.png)",
		"![](assets/rune-intro.gif)",           // BUG4 — fixed
		"![[image.png]]",
		"see [[page|here]] now",
		"plain text with no links",
		"a *italic* and **bold** and ~~strike~~",
		"[ref][id]\n\n[id]: https://example.com",
		"[](https://x.com)",                    // empty-text link (URL as label)
		"[`code`](https://x.com)",              // code inside link
		"***both***",                           // bold+italic
		"a [x](y)* not emphasis",               // lone trailing * must not trip LINK-CLEAN
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
}
