package editor

import (
	"testing"
	"time"

	"rune/internal/editortest"
	"rune/pkg/command"
	"rune/pkg/editor/buffer"
	"rune/pkg/editor/cursor"
	"rune/pkg/editor/history"
	"rune/pkg/editor/keybind"
	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"

	tea "charm.land/bubbletea/v2"
)

func runMultiEditTest(t *testing.T, tc editTestCase) {
	t.Helper()

	st, err := editortest.ParseState(tc.initial)
	if err != nil {
		t.Fatalf("failed to parse initial state: %v", err)
	}

	b := buffer.New(st.Content)
	var cList []cursor.Cursor
	for i, c := range st.Cursors {
		cList = append(cList, cursor.Cursor{Position: c.Position, Anchor: c.Anchor, ID: i + 1})
	}
	cSet := cursor.NewCursorSetFrom(cList)

	ctx := command.CommandContext{
		Buffer:  b,
		Cursors: cSet,
		Args:    tc.args,
	}

	builder := command.NewBuilder()
	builder, err = RegisterCommands(builder)
	if err != nil {
		t.Fatalf("register commands: %v", err)
	}
	reg := builder.Build()

	res := reg.Execute(tc.cmd, ctx)
	if res.Err != nil {
		t.Fatalf("command error: %v", res.Err)
	}

	// Apply the result
	finalContent := st.Content
	finalCursors := cSet

	switch res.Operation.Kind {
	case command.OperationEditBuffer:
		newBuf, _, err := b.ApplyEdits(res.Operation.Edits)
		if err != nil {
			t.Fatalf("apply edits error: %v", err)
		}
		finalContent = newBuf.Content()
		finalCursors = res.Operation.Cursors
	case command.OperationMoveCursors:
		finalCursors = res.Operation.Cursors
	case command.OperationNone:
		// no-op
	default:
		t.Fatalf("unexpected operation kind: %d", res.Operation.Kind)
	}

	var outCursors []editortest.CursorState
	for _, c := range finalCursors.All() {
		outCursors = append(outCursors, editortest.CursorState{Position: c.Position, Anchor: c.Anchor})
	}

	outSt := editortest.TestState{
		Content: finalContent,
		Cursors: outCursors,
	}

	actual := editortest.FormatState(outSt)
	if actual != tc.expected {
		t.Fatalf("mismatch (%s)\ncmd     : %s\ninitial : %q\nexpected: %q\nactual  : %q", tc.name, tc.cmd, tc.initial, tc.expected, actual)
	}
}

func TestSpec_MultiCursorEditing(t *testing.T) {
	tests := []editTestCase{
		// === Multi-cursor insert ===
		{"multi-insert", "a|b|c|d", "edit.insert-character", a("X"), "aX|bX|cX|d"},

		// === Delete with adjacent cursors ===
		{"multi-del-left/spaced", "a|bc|de|f", "edit.delete-left", nil, "|b|d|f"},

		// === Overlapping cursors merge (Gate 2) ===
		{"multi-del-left/merge", "a|b|c", "edit.delete-left", nil, "|c"},

		// === Add cursor below ===
		{"add-below/basic", "hello|\nworld", "multicursor.add-below", nil, "hello|\nworld|"},
		{"add-below/clamps", "hello world|\nhi", "multicursor.add-below", nil, "hello world|\nhi|"},

		// === Escape (Gate 4) ===
		{"escape/multi", "a|b|c", "multicursor.escape", nil, "a|bc"},
		{"escape/sel-only", "h[ell]o", "multicursor.escape", nil, "hell|o"},

		// === Line operations with multi-cursor ===
		{"move-up/basic", "aaa\nb|bb\nccc", "edit.move-line-up", nil, "b|bb\naaa\nccc"},
		{"clone-down/basic", "a|aa\nbbb", "edit.clone-line-down", nil, "a|aa\naaa\nbbb"},

		// === QA Gate 1: 3 cursors insert "X" at offsets [1, 3, 5] in "abcdef" ===
		// "abcdef" with cursors at 1, 3, 5 → "a|bc|de|f"
		// After insert X: "aXbcXdeXf" with cursors at 2, 5, 8
		{"gate1/3-cursor-insert", "a|bc|de|f", "edit.insert-character", a("X"), "aX|bcX|deX|f"},

		// === QA Gate 3: add-below with cursor at end of long line + short next line ===
		// Cursor at col 11 ("hello world" is 11 chars), next line "hi" is 2 chars → clamps to 2
		{"gate3/add-below-clamp", "hello world|\nhi", "multicursor.add-below", nil, "hello world|\nhi|"},

		// === QA Gate 5: Line-range unification: cursors on lines 0 and 1 + move-line-up = no-op ===
		{"gate5/move-up-unified-noop", "a|aa\nb|bb\nccc", "edit.move-line-up", nil, "a|aa\nb|bb\nccc"},
	}

	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) { runMultiEditTest(t, tc) })
	}

	// === QA Gate 6: Multi-cursor edit produces single EditGroup (one undo reverts ALL) ===
	t.Run("gate6/single-undo-group", func(t *testing.T) {
		keys := keymap.Default()
		st := styles.Default()

		builder := command.NewBuilder()
		builder, _ = RegisterCommands(builder)
		reg := builder.Build()
		res, _ := keybind.NewResolver(nil)

		m := New(keys, st, reg, res)
		m = m.SetSize(80, 24)
		m = m.SetFocused(true)
		m.buf = buffer.New("abc")
		m.cursors = cursor.NewCursorSetFrom([]cursor.Cursor{
			{Position: 1, Anchor: 1, ID: 1},
			{Position: 2, Anchor: 2, ID: 2},
			{Position: 3, Anchor: 3, ID: 3},
		})
		m.history = history.New(func() time.Time { return time.Date(2025, 1, 1, 0, 0, 0, 0, time.UTC) })

		// Execute multi-cursor insert
		ctx := command.CommandContext{
			Buffer:  m.buf,
			Cursors: m.cursors,
			Args:    map[string]any{"char": "X"},
		}
		result := reg.Execute("edit.insert-character", ctx)
		if result.Err != nil {
			t.Fatalf("command error: %v", result.Err)
		}

		// Apply via the model's method
		m = m.applyOperation(result.Operation, history.EditBatch, time.Date(2025, 1, 1, 0, 0, 0, 0, time.UTC))

		if m.Content() != "aXbXcX" {
			t.Fatalf("expected content 'aXbXcX', got %q", m.Content())
		}

		// Verify single undo reverts everything
		m, _ = m.applyUndo()
		if m.Content() != "abc" {
			t.Fatalf("expected single undo to restore 'abc', got %q", m.Content())
		}
	})

	// === QA Gate 7: Replacement selections with negative deltas ===
	t.Run("gate7/replacement-sel-negative-delta", func(t *testing.T) {
		// Two selections: "Hello" [0,5) and "World" [6,11) in "Hello World Test"
		// Replace both with "X" → "X X Test", cursors at 1 and 3
		st, err := editortest.ParseState("[Hello] [World] Test")
		if err != nil {
			t.Fatalf("parse: %v", err)
		}

		b := buffer.New(st.Content)
		var cList []cursor.Cursor
		for i, c := range st.Cursors {
			cList = append(cList, cursor.Cursor{Position: c.Position, Anchor: c.Anchor, ID: i + 1})
		}
		cSet := cursor.NewCursorSetFrom(cList)

		ctx := command.CommandContext{
			Buffer:  b,
			Cursors: cSet,
			Args:    map[string]any{"char": "X"},
		}

		builder := command.NewBuilder()
		builder, _ = registerEditCommands(builder)
		reg := builder.Build()

		result := reg.Execute("edit.insert-character", ctx)
		if result.Err != nil {
			t.Fatalf("command error: %v", result.Err)
		}

		newBuf, _, err := b.ApplyEdits(result.Operation.Edits)
		if err != nil {
			t.Fatalf("apply edits: %v", err)
		}

		if newBuf.Content() != "X X Test" {
			t.Fatalf("expected 'X X Test', got %q", newBuf.Content())
		}

		// Check cursor positions: after "X" at pos 1 and after second "X" at pos 3
		all := result.Operation.Cursors.All()
		if len(all) != 2 {
			t.Fatalf("expected 2 cursors, got %d", len(all))
		}
		if all[0].Position != 1 {
			t.Fatalf("cursor 0: expected pos 1, got %d", all[0].Position)
		}
		if all[1].Position != 3 {
			t.Fatalf("cursor 1: expected pos 3, got %d", all[1].Position)
		}
	})

	// === QA Gate 8: Cursor identity preserved through descending edit sort ===
	t.Run("gate8/cursor-identity-preserved", func(t *testing.T) {
		// 3 cursors at offsets 1, 3, 5 in "abcdef"
		// After insert "X", cursors should maintain their original IDs
		b := buffer.New("abcdef")
		cSet := cursor.NewCursorSetFrom([]cursor.Cursor{
			{Position: 1, Anchor: 1, ID: 10},
			{Position: 3, Anchor: 3, ID: 20},
			{Position: 5, Anchor: 5, ID: 30},
		})

		ctx := command.CommandContext{
			Buffer:  b,
			Cursors: cSet,
			Args:    map[string]any{"char": "X"},
		}

		builder := command.NewBuilder()
		builder, _ = registerEditCommands(builder)
		reg := builder.Build()

		result := reg.Execute("edit.insert-character", ctx)
		if result.Err != nil {
			t.Fatalf("command error: %v", result.Err)
		}

		all := result.Operation.Cursors.All()
		if len(all) != 3 {
			t.Fatalf("expected 3 cursors, got %d", len(all))
		}

		// Find cursor with each ID and verify position
		idToPos := map[int]int{}
		for _, c := range all {
			idToPos[c.ID] = c.Position
		}

		// ID 10 was at offset 1, insert X → now at 2
		if pos, ok := idToPos[10]; !ok || pos != 2 {
			t.Fatalf("cursor ID 10: expected pos 2, got %d (found=%v)", pos, ok)
		}
		// ID 20 was at offset 3, +1 from prev insert + own insert → 5
		if pos, ok := idToPos[20]; !ok || pos != 5 {
			t.Fatalf("cursor ID 20: expected pos 5, got %d (found=%v)", pos, ok)
		}
		// ID 30 was at offset 5, +2 from prev inserts + own insert → 8
		if pos, ok := idToPos[30]; !ok || pos != 8 {
			t.Fatalf("cursor ID 30: expected pos 8, got %d (found=%v)", pos, ok)
		}
	})

	// === QA Gate 9: Escape through Update + no physical esc binding ===
	t.Run("gate9/escape-collapses-multicursor", func(t *testing.T) {
		keys := keymap.Default()
		st := styles.Default()

		builder := command.NewBuilder()
		builder, _ = RegisterCommands(builder)
		reg := builder.Build()
		resolver, _ := keybind.NewResolver(nil)

		m := New(keys, st, reg, resolver)
		m = m.SetSize(80, 24)
		m = m.SetFocused(true)
		m.buf = buffer.New("abc")
		m.cursors = cursor.NewCursorSetFrom([]cursor.Cursor{
			{Position: 1, Anchor: 1, ID: 1},
			{Position: 2, Anchor: 2, ID: 2},
		})
		m.history = history.New(func() time.Time { return time.Now() })

		// Send Escape key
		m, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyEscape})

		// Should collapse to single cursor (primary = lowest offset)
		if m.cursors.Len() != 1 {
			t.Fatalf("expected 1 cursor after Escape, got %d", m.cursors.Len())
		}
		if m.cursors.Primary().Position != 1 {
			t.Fatalf("expected primary cursor at 1, got %d", m.cursors.Primary().Position)
		}
	})

	t.Run("gate9/escape-collapses-selection", func(t *testing.T) {
		keys := keymap.Default()
		st := styles.Default()

		builder := command.NewBuilder()
		builder, _ = RegisterCommands(builder)
		reg := builder.Build()
		resolver, _ := keybind.NewResolver(nil)

		m := New(keys, st, reg, resolver)
		m = m.SetSize(80, 24)
		m = m.SetFocused(true)
		m.buf = buffer.New("hello")
		// Selection: "ell" [1,4) — cursor at 4, anchor at 1
		m.cursors = cursor.NewCursorSetFrom([]cursor.Cursor{
			{Position: 4, Anchor: 1, ID: 1},
		})
		m.history = history.New(func() time.Time { return time.Now() })

		// Send Escape key
		m, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyEscape})

		// Should collapse selection (cursor stays at Position)
		if m.cursors.Primary().HasSelection() {
			t.Fatalf("expected no selection after Escape")
		}
		if m.cursors.Primary().Position != 4 {
			t.Fatalf("expected cursor at position 4, got %d", m.cursors.Primary().Position)
		}
	})

	t.Run("gate9/no-esc-in-command-bindings", func(t *testing.T) {
		keys := keymap.Default()
		bindings, err := keys.CommandBindings()
		if err != nil {
			t.Fatalf("CommandBindings error: %v", err)
		}

		for _, b := range bindings {
			for _, chord := range b.Chords {
				if chord.Key == "esc" || chord.Key == "escape" {
					t.Fatalf("CommandBindings() contains physical 'esc' binding for command %q", b.Command)
				}
			}
		}
	})
}
