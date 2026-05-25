package editor

import (
			"testing"
	
	"rune/internal/editortest"
	"rune/pkg/command"
	"rune/pkg/editor/buffer"
	"rune/pkg/editor/coords"
	"rune/pkg/editor/cursor"
)

type navTestCase struct {
	name     string
	initial  string
	cmd      string
	expected string
	wrap     bool
}

func runNavTest(t *testing.T, tc navTestCase) {
	st, err := editortest.ParseState(tc.initial)
	if err != nil {
		t.Fatalf("failed to parse initial state: %v", err)
	}
	
	b := buffer.New(st.Content)
	var cList []cursor.Cursor
	for i, c := range st.Cursors {
		sp := coords.SyntaxPoint{Line: b.OffsetToLineCol(c.Position).Line, Col: b.OffsetToLineCol(c.Position).Col}
		cList = append(cList, cursor.Cursor{Position: c.Position, Anchor: c.Anchor, ID: i, DesiredCol: sp.Col})
	}
	cSet := cursor.NewCursorSetFrom(cList)

	ctx := command.CommandContext{
		Buffer:  b,
		Cursors: cSet,
		BufferToSyntax: func(bp coords.BufferPoint) coords.SyntaxPoint {
			return coords.SyntaxPoint{Line: bp.Line, Col: bp.Col}
		},
		SyntaxToBuffer: func(sp coords.SyntaxPoint) coords.BufferPoint {
			return coords.BufferPoint{Line: sp.Line, Col: sp.Col}
		},
		SyntaxToWrap: func(sp coords.SyntaxPoint) coords.WrapPoint {
			return coords.WrapPoint{Row: sp.Line, Col: sp.Col}
		},
		WrapToSyntax: func(wp coords.WrapPoint) coords.SyntaxPoint {
			return coords.SyntaxPoint{Line: wp.Row, Col: wp.Col}
		},
		SoftWrap: func() bool { return tc.wrap },
		TotalRows: func() int { return b.LineCount() },
		ViewportHeight: func() int { return 10 },
	}

	builder := command.NewBuilder()
	builder, _ = registerNavCommands(builder)
	reg := builder.Build()

	res := reg.Execute(tc.cmd, ctx)
	if res.Err != nil {
		t.Fatalf("command error: %v", res.Err)
	}

	if res.Operation.Kind == command.OperationMoveCursors {
		cSet = res.Operation.Cursors
	} else if res.Operation.Kind == command.OperationScroll {
        return
    }
	
	var outCursors []editortest.CursorState
	for _, c := range cSet.All() {
		outCursors = append(outCursors, editortest.CursorState{Position: c.Position, Anchor: c.Anchor})
	}
	
	outSt := editortest.TestState{
		Content: st.Content,
		Cursors: outCursors,
	}
	
	actual := editortest.FormatState(outSt)
	if actual != tc.expected {
		t.Fatalf("mismatch (%s)\ncmd     : %s\ninitial : %q\nexpected: %q\nactual  : %q\n", tc.name, tc.cmd, tc.initial, tc.expected, actual)
	}
}


func TestSpec_Navigation(t *testing.T) {
	tests := []navTestCase{
		{"left/mid", "hel|lo", "cursor.character-left", "he|llo", false},
		{"left/doc-start", "|hello", "cursor.character-left", "|hello", false},
		{"left/wrap", "hello\n|world", "cursor.character-left", "hello|\nworld", false},
		{"left/col-fwd", "he[ll]o", "cursor.character-left", "he|llo", false},
		{"left/col-back", "he]ll[o", "cursor.character-left", "he|llo", false},
		{"left/multi", "a|b|c", "cursor.character-left", "|a|bc", false},
		{"sel-left/mid", "hel|lo", "select.character-left", "he]l[lo", false},
		{"sel-left/extend-fwd", "he[ll]o", "select.character-left", "he[l]lo", false},
		{"sel-left/extend-back", "he]ll[o", "select.character-left", "h]ell[o", false},
		{"sel-left/doc-start", "]h[ello", "select.character-left", "]h[ello", false},
		{"right/mid", "he|llo", "cursor.character-right", "hel|lo", false},
		{"right/doc-end", "hello|", "cursor.character-right", "hello|", false},
		{"right/wrap", "hello|\nworld", "cursor.character-right", "hello\n|world", false},
		{"right/col-fwd", "he[ll]o", "cursor.character-right", "hell|o", false},
		{"right/col-back", "he]ll[o", "cursor.character-right", "hell|o", false},
		{"right/multi", "a|b|c", "cursor.character-right", "ab|c|", false},
		{"sel-right/mid", "he|llo", "select.character-right", "he[l]lo", false},
		{"sel-right/extend-fwd", "he[l]lo", "select.character-right", "he[ll]o", false},
		{"sel-right/extend-back", "he]ll[o", "select.character-right", "hel]l[o", false},
		{"sel-right/doc-end", "hell[o]", "select.character-right", "hell[o]", false},
		{"up/mid", "aa\nb|b", "cursor.line-up", "a|a\nbb", false},
		{"up/doc-start", "|bb", "cursor.line-up", "|bb", false},
		{"up/long-to-short", "a\nbb|b", "cursor.line-up", "a|\nbbb", false},
		{"up/col-fwd", "a\nb[c]d", "cursor.line-up", "a|\nbcd", false},
		{"up/multi", "a|a\nb|b", "cursor.line-up", "|a|a\nbb", false},
		{"sel-up/mid", "aa\nb|b", "select.line-up", "a]a\nb[b", false},
		{"sel-up/doc-start", "a]a[", "select.line-up", "]aa[", false},
		{"sel-up/multi", "a\n|b\nc\n|d", "select.line-up", "]a\n[b\n]c\n[d", false},
		{"down/mid", "a|a\nbb", "cursor.line-down", "aa\nb|b", false},
		{"down/doc-end", "a|a", "cursor.line-down", "aa|", false},
		{"down/long-to-short", "aa|a\nb", "cursor.line-down", "aaa\nb|", false},
		{"down/col-fwd", "a[a]a\nb", "cursor.line-down", "aaa\nb|", false},
		{"down/multi", "a|a\nb|b", "cursor.line-down", "aa\nb|b|", false},
		{"sel-down/mid", "a|a\nbb", "select.line-down", "a[a\nb]b", false},
		{"sel-down/doc-end", "a[a]", "select.line-down", "a[a]", false},
		{"sel-down/short", "a|a\nb", "select.line-down", "a[a\nb]", false},
		{"word-left/mid", "hel|lo", "cursor.word-left", "|hello", false},
		{"word-left/space", "hello  |world", "cursor.word-left", "|hello  world", false},
		{"word-left/doc-start", "|hello", "cursor.word-left", "|hello", false},
		{"word-left/col-fwd", "he[ll]o", "cursor.word-left", "|hello", false},
		{"word-left/utf8", "café| world", "cursor.word-left", "caf|é world", false},
		{"word-left/multi", "h|ello w|orld", "cursor.word-left", "|hello |world", false},
		{"sel-word-left/mid", "hel|lo", "select.word-left", "]hel[lo", false},
		{"sel-word-left/space", "hello  |world", "select.word-left", "]hello  [world", false},
		{"sel-word-left/extend", "he]l[lo", "select.word-left", "]hel[lo", false},
		{"word-right/mid", "he|llo", "cursor.word-right", "hello|", false},
		{"word-right/space", "hello|  world", "cursor.word-right", "hello  world|", false},
		{"word-right/doc-end", "hello|", "cursor.word-right", "hello|", false},
		{"word-right/col-fwd", "he[ll]o", "cursor.word-right", "hello|", false},
		{"word-right/multi", "h|ello w|orld", "cursor.word-right", "hello| world|", false},
		{"word-right/utf8", "caf|é world", "cursor.word-right", "café| world", false},
		{"sel-word-right/mid", "he|llo", "select.word-right", "he[llo]", false},
		{"sel-word-right/space", "hello|  world", "select.word-right", "hello[  world]", false},
		{"sel-word-right/extend", "he[l]lo", "select.word-right", "he[llo]", false},
		{"line-start/mid", "  he|llo", "cursor.line-start", "  |hello", false},
		{"line-start/toggle", "  |hello", "cursor.line-start", "|  hello", false},
		{"line-start/toggle-back", "|  hello", "cursor.line-start", "  |hello", false},
		{"line-start/doc-start", "|hello", "cursor.line-start", "|hello", false},
		{"sel-line-start/mid", "  h|ello", "select.line-start", "  ]h[ello", false},
		{"sel-line-start/toggle", "  ]h[ello", "select.line-start", "]  h[ello", false},
		{"line-end/mid", "h|ello", "cursor.line-end", "hello|", false},
		{"line-end/nl", "h|ello\n", "cursor.line-end", "hello|\n", false},
		{"line-end/doc-end", "hello|", "cursor.line-end", "hello|", false},
		{"line-end/multi", "h|i\nt|here", "cursor.line-end", "hi|\nthere|", false},
		{"sel-line-end/mid", "h|ello", "select.line-end", "h[ello]", false},
		{"sel-line-end/nl", "h|ello\n", "select.line-end", "h[ello]\n", false},
		{"doc-start/mid", "a\n|b", "cursor.document-start", "|a\nb", false},
		{"doc-start/doc-start", "|a", "cursor.document-start", "|a", false},
		{"doc-start/col-fwd", "a\n[b]", "cursor.document-start", "|a\nb", false},
		{"sel-doc-start/mid", "a\n|b", "select.document-start", "]a\n[b", false},
		{"sel-doc-start/doc-start", "]a[", "select.document-start", "]a[", false},
		{"doc-end/mid", "a|\nb", "cursor.document-end", "a\nb|", false},
		{"doc-end/doc-end", "a\nb|", "cursor.document-end", "a\nb|", false},
		{"doc-end/col-fwd", "[a]\nb", "cursor.document-end", "a\nb|", false},
		{"sel-doc-end/mid", "a|\nb", "select.document-end", "a[\nb]", false},
		{"sel-doc-end/doc-end", "a[b]", "select.document-end", "a[b]", false},
		{"page-up/mid", "1\n2\n3\n4\n5\n6\n7\n8\n9\n1|0\n11\n12", "cursor.page-up", "1\n2|\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12", false},
		{"page-up/doc-start", "1\n2|\n3", "cursor.page-up", "|1\n2\n3", false},
		{"page-up/col-fwd", "1\n2\n3\n4\n5\n6\n7\n8\n9\n1[0]\n11", "cursor.page-up", "1\n2|\n3\n4\n5\n6\n7\n8\n9\n10\n11", false},
		{"sel-page-up/mid", "1\n2\n3\n4\n5\n6\n7\n8\n9\n1|0\n11\n12", "select.page-up", "1\n2]\n3\n4\n5\n6\n7\n8\n9\n1[0\n11\n12", false},
		{"sel-page-up/doc-start", "1\n2|\n3", "select.page-up", "]1\n2[\n3", false},
		{"page-down/mid", "1\n|2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12", "cursor.page-down", "1\n2\n3\n4\n5\n6\n7\n8\n9\n|10\n11\n12", false},
		{"page-down/doc-end", "1\n2\n3\n4\n5\n6\n7\n8\n9\n|10", "cursor.page-down", "1\n2\n3\n4\n5\n6\n7\n8\n9\n10|", false},
		{"page-down/col-fwd", "1\n[2]\n3\n4\n5\n6\n7\n8\n9\n10", "cursor.page-down", "1\n2\n3\n4\n5\n6\n7\n8\n9\n1|0", false},
		{"sel-page-down/mid", "1\n|2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12", "select.page-down", "1\n[2\n3\n4\n5\n6\n7\n8\n9\n]10\n11\n12", false},
		{"sel-page-down/doc-end", "1\n2\n3\n4\n5\n6\n7\n8\n9\n|10", "select.page-down", "1\n2\n3\n4\n5\n6\n7\n8\n9\n[10]", false},
		{"select.all/mid", "a|b", "select.all", "[ab]", false},
		{"scroll.line-up", "a\n|b\nc", "scroll.line-up", "a\n|b\nc", false},
		{"scroll.line-down", "a\n|b\nc", "scroll.line-down", "a\n|b\nc", false},
		{"scroll.character-left", "a\n|b\nc", "scroll.character-left", "a\n|b\nc", false},
		{"scroll.character-right", "a\n|b\nc", "scroll.character-right", "a\n|b\nc", false},
	}
	for _, tc := range tests {
		if len(tc.cmd) < 6 || tc.cmd[:6] != "select" {
			t.Run(tc.name, func(t *testing.T) { runNavTest(t, tc) })
		}
	}
}

func TestSpec_SelectionNavigation(t *testing.T) {
	tests := []navTestCase{
		{"left/mid", "hel|lo", "cursor.character-left", "he|llo", false},
		{"left/doc-start", "|hello", "cursor.character-left", "|hello", false},
		{"left/wrap", "hello\n|world", "cursor.character-left", "hello|\nworld", false},
		{"left/col-fwd", "he[ll]o", "cursor.character-left", "he|llo", false},
		{"left/col-back", "he]ll[o", "cursor.character-left", "he|llo", false},
		{"left/multi", "a|b|c", "cursor.character-left", "|a|bc", false},
		{"sel-left/mid", "hel|lo", "select.character-left", "he]l[lo", false},
		{"sel-left/extend-fwd", "he[ll]o", "select.character-left", "he[l]lo", false},
		{"sel-left/extend-back", "he]ll[o", "select.character-left", "h]ell[o", false},
		{"sel-left/doc-start", "]h[ello", "select.character-left", "]h[ello", false},
		{"right/mid", "he|llo", "cursor.character-right", "hel|lo", false},
		{"right/doc-end", "hello|", "cursor.character-right", "hello|", false},
		{"right/wrap", "hello|\nworld", "cursor.character-right", "hello\n|world", false},
		{"right/col-fwd", "he[ll]o", "cursor.character-right", "hell|o", false},
		{"right/col-back", "he]ll[o", "cursor.character-right", "hell|o", false},
		{"right/multi", "a|b|c", "cursor.character-right", "ab|c|", false},
		{"sel-right/mid", "he|llo", "select.character-right", "he[l]lo", false},
		{"sel-right/extend-fwd", "he[l]lo", "select.character-right", "he[ll]o", false},
		{"sel-right/extend-back", "he]ll[o", "select.character-right", "hel]l[o", false},
		{"sel-right/doc-end", "hell[o]", "select.character-right", "hell[o]", false},
		{"up/mid", "aa\nb|b", "cursor.line-up", "a|a\nbb", false},
		{"up/doc-start", "|bb", "cursor.line-up", "|bb", false},
		{"up/long-to-short", "a\nbb|b", "cursor.line-up", "a|\nbbb", false},
		{"up/col-fwd", "a\nb[c]d", "cursor.line-up", "a|\nbcd", false},
		{"up/multi", "a|a\nb|b", "cursor.line-up", "|a|a\nbb", false},
		{"sel-up/mid", "aa\nb|b", "select.line-up", "a]a\nb[b", false},
		{"sel-up/doc-start", "a]a[", "select.line-up", "]aa[", false},
		{"sel-up/multi", "a\n|b\nc\n|d", "select.line-up", "]a\n[b\n]c\n[d", false},
		{"down/mid", "a|a\nbb", "cursor.line-down", "aa\nb|b", false},
		{"down/doc-end", "a|a", "cursor.line-down", "aa|", false},
		{"down/long-to-short", "aa|a\nb", "cursor.line-down", "aaa\nb|", false},
		{"down/col-fwd", "a[a]a\nb", "cursor.line-down", "aaa\nb|", false},
		{"down/multi", "a|a\nb|b", "cursor.line-down", "aa\nb|b|", false},
		{"sel-down/mid", "a|a\nbb", "select.line-down", "a[a\nb]b", false},
		{"sel-down/doc-end", "a[a]", "select.line-down", "a[a]", false},
		{"sel-down/short", "a|a\nb", "select.line-down", "a[a\nb]", false},
		{"word-left/mid", "hel|lo", "cursor.word-left", "|hello", false},
		{"word-left/space", "hello  |world", "cursor.word-left", "|hello  world", false},
		{"word-left/doc-start", "|hello", "cursor.word-left", "|hello", false},
		{"word-left/col-fwd", "he[ll]o", "cursor.word-left", "|hello", false},
		{"word-left/utf8", "café| world", "cursor.word-left", "caf|é world", false},
		{"word-left/multi", "h|ello w|orld", "cursor.word-left", "|hello |world", false},
		{"sel-word-left/mid", "hel|lo", "select.word-left", "]hel[lo", false},
		{"sel-word-left/space", "hello  |world", "select.word-left", "]hello  [world", false},
		{"sel-word-left/extend", "he]l[lo", "select.word-left", "]hel[lo", false},
		{"word-right/mid", "he|llo", "cursor.word-right", "hello|", false},
		{"word-right/space", "hello|  world", "cursor.word-right", "hello  world|", false},
		{"word-right/doc-end", "hello|", "cursor.word-right", "hello|", false},
		{"word-right/col-fwd", "he[ll]o", "cursor.word-right", "hello|", false},
		{"word-right/multi", "h|ello w|orld", "cursor.word-right", "hello| world|", false},
		{"word-right/utf8", "caf|é world", "cursor.word-right", "café| world", false},
		{"sel-word-right/mid", "he|llo", "select.word-right", "he[llo]", false},
		{"sel-word-right/space", "hello|  world", "select.word-right", "hello[  world]", false},
		{"sel-word-right/extend", "he[l]lo", "select.word-right", "he[llo]", false},
		{"line-start/mid", "  he|llo", "cursor.line-start", "  |hello", false},
		{"line-start/toggle", "  |hello", "cursor.line-start", "|  hello", false},
		{"line-start/toggle-back", "|  hello", "cursor.line-start", "  |hello", false},
		{"line-start/doc-start", "|hello", "cursor.line-start", "|hello", false},
		{"sel-line-start/mid", "  h|ello", "select.line-start", "  ]h[ello", false},
		{"sel-line-start/toggle", "  ]h[ello", "select.line-start", "]  h[ello", false},
		{"line-end/mid", "h|ello", "cursor.line-end", "hello|", false},
		{"line-end/nl", "h|ello\n", "cursor.line-end", "hello|\n", false},
		{"line-end/doc-end", "hello|", "cursor.line-end", "hello|", false},
		{"line-end/multi", "h|i\nt|here", "cursor.line-end", "hi|\nthere|", false},
		{"sel-line-end/mid", "h|ello", "select.line-end", "h[ello]", false},
		{"sel-line-end/nl", "h|ello\n", "select.line-end", "h[ello]\n", false},
		{"doc-start/mid", "a\n|b", "cursor.document-start", "|a\nb", false},
		{"doc-start/doc-start", "|a", "cursor.document-start", "|a", false},
		{"doc-start/col-fwd", "a\n[b]", "cursor.document-start", "|a\nb", false},
		{"sel-doc-start/mid", "a\n|b", "select.document-start", "]a\n[b", false},
		{"sel-doc-start/doc-start", "]a[", "select.document-start", "]a[", false},
		{"doc-end/mid", "a|\nb", "cursor.document-end", "a\nb|", false},
		{"doc-end/doc-end", "a\nb|", "cursor.document-end", "a\nb|", false},
		{"doc-end/col-fwd", "[a]\nb", "cursor.document-end", "a\nb|", false},
		{"sel-doc-end/mid", "a|\nb", "select.document-end", "a[\nb]", false},
		{"sel-doc-end/doc-end", "a[b]", "select.document-end", "a[b]", false},
		{"page-up/mid", "1\n2\n3\n4\n5\n6\n7\n8\n9\n1|0\n11\n12", "cursor.page-up", "1\n2|\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12", false},
		{"page-up/doc-start", "1\n2|\n3", "cursor.page-up", "|1\n2\n3", false},
		{"page-up/col-fwd", "1\n2\n3\n4\n5\n6\n7\n8\n9\n1[0]\n11", "cursor.page-up", "1\n2|\n3\n4\n5\n6\n7\n8\n9\n10\n11", false},
		{"sel-page-up/mid", "1\n2\n3\n4\n5\n6\n7\n8\n9\n1|0\n11\n12", "select.page-up", "1\n2]\n3\n4\n5\n6\n7\n8\n9\n1[0\n11\n12", false},
		{"sel-page-up/doc-start", "1\n2|\n3", "select.page-up", "]1\n2[\n3", false},
		{"page-down/mid", "1\n|2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12", "cursor.page-down", "1\n2\n3\n4\n5\n6\n7\n8\n9\n|10\n11\n12", false},
		{"page-down/doc-end", "1\n2\n3\n4\n5\n6\n7\n8\n9\n|10", "cursor.page-down", "1\n2\n3\n4\n5\n6\n7\n8\n9\n10|", false},
		{"page-down/col-fwd", "1\n[2]\n3\n4\n5\n6\n7\n8\n9\n10", "cursor.page-down", "1\n2\n3\n4\n5\n6\n7\n8\n9\n1|0", false},
		{"sel-page-down/mid", "1\n|2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12", "select.page-down", "1\n[2\n3\n4\n5\n6\n7\n8\n9\n]10\n11\n12", false},
		{"sel-page-down/doc-end", "1\n2\n3\n4\n5\n6\n7\n8\n9\n|10", "select.page-down", "1\n2\n3\n4\n5\n6\n7\n8\n9\n[10]", false},
		{"select.all/mid", "a|b", "select.all", "[ab]", false},
		{"scroll.line-up", "a\n|b\nc", "scroll.line-up", "a\n|b\nc", false},
		{"scroll.line-down", "a\n|b\nc", "scroll.line-down", "a\n|b\nc", false},
		{"scroll.character-left", "a\n|b\nc", "scroll.character-left", "a\n|b\nc", false},
		{"scroll.character-right", "a\n|b\nc", "scroll.character-right", "a\n|b\nc", false},
	}
	for _, tc := range tests {
		if len(tc.cmd) >= 6 && tc.cmd[:6] == "select" {
			t.Run(tc.name, func(t *testing.T) { runNavTest(t, tc) })
		}
	}
}
