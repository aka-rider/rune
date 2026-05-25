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
	"rune/pkg/terminal"
	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"
)

// newTestEditor returns an editor Model wired for deterministic testing.
func newTestEditor(content string) Model {
	keys := keymap.Default()
	st := styles.Default()
	builder := command.NewBuilder()
	builder, _ = RegisterCommands(builder)
	reg := builder.Build()
	resolver, _ := keybind.NewResolver(nil)

	m := New(keys, st, reg, resolver, terminal.TermCaps{})
	m = m.SetSize(80, 24)
	m = m.SetFocused(true)
	m.buf = buffer.New(content)
	m.cursors = cursor.NewCursorSet(0)
	return m
}

// newTestEditorFromNotation creates a test editor from notation string.
func newTestEditorFromNotation(notation string) Model {
	st, err := editortest.ParseState(notation)
	if err != nil {
		panic("bad notation: " + err.Error())
	}
	m := newTestEditor(st.Content)
	var cList []cursor.Cursor
	for i, c := range st.Cursors {
		cList = append(cList, cursor.Cursor{Position: c.Position, Anchor: c.Anchor, ID: i + 1})
	}
	m.cursors = cursor.NewCursorSetFrom(cList)
	return m
}

// formatModelState formats editor state back to notation.
func formatModelState(m Model) string {
	var outCursors []editortest.CursorState
	for _, c := range m.cursors.All() {
		outCursors = append(outCursors, editortest.CursorState{Position: c.Position, Anchor: c.Anchor})
	}
	return editortest.FormatState(editortest.TestState{
		Content: m.Content(),
		Cursors: outCursors,
	})
}

// applyCmd executes a named command with given args on the model at the given time.
func applyCmd(m Model, cmdName string, args map[string]any, now time.Time) Model {
	ctx := command.CommandContext{
		Buffer:  m.buf,
		Cursors: m.cursors,
		Args:    args,
	}
	res := m.registry.Execute(cmdName, ctx)
	if res.Err != nil {
		return m
	}
	if res.Operation.Kind == command.OperationNone {
		return m
	}
	m = m.applyOperation(res.Operation, m.editKindFromCommand(cmdName), now)
	m = m.syncDisplay()
	return m
}

// --- TestSpec_UndoCoalescing ---

func TestSpec_UndoCoalescing(t *testing.T) {
	tests := []struct {
		name string
		ops  []struct {
			char    string
			deltaMs int64
		}
		wantUndos int
	}{
		{
			name: "coalesce-fast-typing",
			ops: []struct {
				char    string
				deltaMs int64
			}{
				{"a", 0},
				{"b", 100},
				{"c", 200},
			},
			wantUndos: 1,
		},
		{
			name: "break-on-idle",
			ops: []struct {
				char    string
				deltaMs int64
			}{
				{"a", 0},
				{"b", 400},
			},
			wantUndos: 2,
		},
		{
			name: "break-on-whitespace",
			ops: []struct {
				char    string
				deltaMs int64
			}{
				{"a", 0},
				{" ", 50},
			},
			wantUndos: 2,
		},
		{
			name: "break-on-delete",
			ops: []struct {
				char    string
				deltaMs int64
			}{
				{"a", 0},
				{"DELETE", 50},
			},
			wantUndos: 2,
		},
	}

	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			m := newTestEditor("")
			m.cursors = cursor.NewCursorSet(0)
			clk := editortest.NewClock()
			m.history = history.New(clk.Now)

			for _, op := range tc.ops {
				clk = clk.Advance(time.Duration(op.deltaMs) * time.Millisecond)
				now := clk.Now()

				if op.char == "DELETE" {
					m = applyCmd(m, "edit.delete-left", nil, now)
				} else {
					m = applyCmd(m, "edit.insert-character", a(op.char), now)
				}
			}

			// Count undos by undoing until empty
			undoCount := 0
			for m.history.CanUndo() {
				m, _ = m.applyUndo()
				undoCount++
			}

			if undoCount != tc.wantUndos {
				t.Fatalf("expected %d undo groups, got %d", tc.wantUndos, undoCount)
			}
		})
	}
}

// --- TestSpec_UndoRestoresCursors ---

func TestSpec_UndoRestoresCursors(t *testing.T) {
	tests := []struct {
		name     string
		initial  string
		cmd      string
		args     map[string]any
		expected string // state after undo should match initial
	}{
		{"undo-insert", "hel|lo", "edit.insert-character", a("X"), "hel|lo"},
		{"undo-delete", "hel|lo", "edit.delete-left", nil, "hel|lo"},
		{"undo-multi", "a|b|c", "edit.insert-character", a("X"), "a|b|c"},
	}

	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			m := newTestEditorFromNotation(tc.initial)
			clk := editortest.NewClock()
			m.history = history.New(clk.Now)

			now := clk.Now()
			m = applyCmd(m, tc.cmd, tc.args, now)

			// Undo
			m, _ = m.applyUndo()
			m = m.syncDisplay()

			actual := formatModelState(m)
			if actual != tc.expected {
				t.Fatalf("after undo:\nexpected: %q\nactual:   %q", tc.expected, actual)
			}
		})
	}
}

// --- TestSpec_RedoClearedOnNewEdit ---

func TestSpec_RedoClearedOnNewEdit(t *testing.T) {
	m := newTestEditor("hello")
	m.cursors = cursor.NewCursorSet(5)
	clk := editortest.NewClock()
	m.history = history.New(clk.Now)

	// Insert "X"
	now := clk.Now()
	m = applyCmd(m, "edit.insert-character", a("X"), now)

	if m.Content() != "helloX" {
		t.Fatalf("expected helloX, got %q", m.Content())
	}

	// Undo
	m, _ = m.applyUndo()
	if m.Content() != "hello" {
		t.Fatalf("after undo expected hello, got %q", m.Content())
	}
	if !m.history.CanRedo() {
		t.Fatalf("expected CanRedo==true after undo")
	}

	// New edit (should clear redo)
	clk = clk.Advance(500 * time.Millisecond)
	now = clk.Now()
	m = applyCmd(m, "edit.insert-character", a("Y"), now)

	if m.history.CanRedo() {
		t.Fatalf("expected CanRedo==false after new edit, got true")
	}
}

// --- TestSpec_Undo — comprehensive QA Gates ---

func TestSpec_Undo(t *testing.T) {
	// Gate 1: Trust test — 20 varied operations + undo ALL → byte-identical to original
	t.Run("gate1/trust-test-20-ops-undo-all", func(t *testing.T) {
		original := "line one\nline two\nline three\nline four\n"
		m := newTestEditor(original)
		m.cursors = cursor.NewCursorSet(0)
		clk := editortest.NewClock()
		m.history = history.New(clk.Now)

		originalCursors := m.cursors.All()

		// Apply 20 varied operations with time gaps to prevent coalescing
		ops := []struct {
			cmd  string
			args map[string]any
		}{
			{"edit.insert-character", a("A")},
			{"edit.insert-character", a("B")},
			{"edit.insert-character", a("C")},
			{"edit.delete-left", nil},
			{"edit.insert-character", a("D")},
			{"edit.newline", nil},
			{"edit.insert-character", a("E")},
			{"edit.insert-character", a("F")},
			{"edit.delete-left", nil},
			{"edit.insert-character", a("G")},
			{"edit.insert-character", a(" ")},
			{"edit.insert-character", a("H")},
			{"edit.delete-left", nil},
			{"edit.insert-character", a("I")},
			{"edit.newline", nil},
			{"edit.insert-character", a("J")},
			{"edit.insert-character", a("K")},
			{"edit.delete-left", nil},
			{"edit.insert-character", a("L")},
			{"edit.insert-character", a("M")},
		}

		for _, op := range ops {
			clk = clk.Advance(500 * time.Millisecond) // prevent coalescing
			now := clk.Now()
			m = applyCmd(m, op.cmd, op.args, now)
		}

		// Undo everything
		for m.history.CanUndo() {
			m, _ = m.applyUndo()
		}
		m = m.syncDisplay()

		if m.Content() != original {
			t.Fatalf("content not restored after undoing all\nexpected: %q\nactual:   %q", original, m.Content())
		}

		// Cursor state should be at position 0 (original)
		if len(m.cursors.All()) != len(originalCursors) {
			t.Fatalf("cursor count mismatch: expected %d, got %d", len(originalCursors), len(m.cursors.All()))
		}
		if m.cursors.Primary().Position != originalCursors[0].Position {
			t.Fatalf("cursor position not restored: expected %d, got %d", originalCursors[0].Position, m.cursors.Primary().Position)
		}
	})

	// Gate 2: Undo across cursor-count change
	t.Run("gate2/undo-across-cursor-count-change", func(t *testing.T) {
		m := newTestEditorFromNotation("a|b|c|")
		clk := editortest.NewClock()
		m.history = history.New(clk.Now)

		initialState := formatModelState(m)

		// Insert X at each cursor (3 cursors)
		now := clk.Now()
		m = applyCmd(m, "edit.insert-character", a("X"), now)

		// Verify we have cursors after the edit
		if m.cursors.Len() < 1 {
			t.Fatalf("expected cursors after edit")
		}

		// Undo → should restore 3 cursors at original positions
		m, _ = m.applyUndo()
		m = m.syncDisplay()

		actual := formatModelState(m)
		if actual != initialState {
			t.Fatalf("undo didn't restore cursor state\nexpected: %q\nactual:   %q", initialState, actual)
		}
	})

	// Gate 3: Multi-cursor edit = 1 undo group
	t.Run("gate3/multi-cursor-single-undo-group", func(t *testing.T) {
		m := newTestEditorFromNotation("a|b|c|")
		clk := editortest.NewClock()
		m.history = history.New(clk.Now)

		// 3 cursors insert "X" → should be single undo group
		now := clk.Now()
		m = applyCmd(m, "edit.insert-character", a("X"), now)

		// Single undo should revert all 3 insertions
		m, _ = m.applyUndo()
		m = m.syncDisplay()

		if m.Content() != "abc" {
			t.Fatalf("single undo didn't revert all multi-cursor inserts, got %q", m.Content())
		}

		// Should not be able to undo further
		if m.history.CanUndo() {
			t.Fatalf("expected only 1 undo group for multi-cursor edit")
		}
	})

	// Gate 4: Whitespace insertion breaks coalescing
	t.Run("gate4/whitespace-breaks-coalescing", func(t *testing.T) {
		m := newTestEditor("")
		m.cursors = cursor.NewCursorSet(0)
		clk := editortest.NewClock()
		m.history = history.New(clk.Now)

		// Type "a", "b", then " " with fast typing
		now := clk.Now()
		m = applyCmd(m, "edit.insert-character", a("a"), now)

		clk = clk.Advance(50 * time.Millisecond)
		now = clk.Now()
		m = applyCmd(m, "edit.insert-character", a("b"), now)

		clk = clk.Advance(50 * time.Millisecond)
		now = clk.Now()
		m = applyCmd(m, "edit.insert-character", a(" "), now)

		if m.Content() != "ab " {
			t.Fatalf("expected 'ab ', got %q", m.Content())
		}

		// First undo removes " "
		m, _ = m.applyUndo()
		if m.Content() != "ab" {
			t.Fatalf("after first undo expected 'ab', got %q", m.Content())
		}

		// Second undo removes "ab"
		m, _ = m.applyUndo()
		if m.Content() != "" {
			t.Fatalf("after second undo expected empty, got %q", m.Content())
		}
	})

	// Gate 5: Redo invalidation
	t.Run("gate5/redo-invalidation", func(t *testing.T) {
		m := newTestEditor("hello")
		m.cursors = cursor.NewCursorSet(5)
		clk := editortest.NewClock()
		m.history = history.New(clk.Now)

		// Make 3 edits
		for i, ch := range []string{"A", "B", "C"} {
			clk = clk.Advance(500 * time.Millisecond)
			_ = i
			m = applyCmd(m, "edit.insert-character", a(ch), clk.Now())
		}

		// Undo 2
		m, _ = m.applyUndo()
		m, _ = m.applyUndo()

		if !m.history.CanRedo() {
			t.Fatalf("expected CanRedo==true after undos")
		}

		// Make any new edit
		clk = clk.Advance(500 * time.Millisecond)
		m = applyCmd(m, "edit.insert-character", a("Z"), clk.Now())

		if m.history.CanRedo() {
			t.Fatalf("expected CanRedo==false after new edit")
		}
	})

	// Gate 6: Undo of move-line-up restores content AND cursor position
	t.Run("gate6/undo-move-line-up", func(t *testing.T) {
		m := newTestEditorFromNotation("aaa\nb|bb\nccc")
		clk := editortest.NewClock()
		m.history = history.New(clk.Now)

		initialState := formatModelState(m)

		now := clk.Now()
		m = applyCmd(m, "edit.move-line-up", nil, now)

		// Verify move happened
		if m.Content() == "aaa\nbbb\nccc" {
			t.Fatalf("move-line-up didn't change content")
		}

		// Undo
		m, _ = m.applyUndo()
		m = m.syncDisplay()

		actual := formatModelState(m)
		if actual != initialState {
			t.Fatalf("undo of move-line-up didn't restore state\nexpected: %q\nactual:   %q", initialState, actual)
		}
	})

	// Gate 7: Undo back to saved bytes → dirty reflects content equality
	t.Run("gate7/undo-to-saved-state-clean", func(t *testing.T) {
		m := newTestEditor("hello")
		m.cursors = cursor.NewCursorSet(5)
		m.savedContentHash = hashContent("hello")
		m.dirty = false
		clk := editortest.NewClock()
		m.history = history.New(clk.Now)

		// Edit
		clk = clk.Advance(500 * time.Millisecond)
		m = applyCmd(m, "edit.insert-character", a("X"), clk.Now())

		if !m.IsDirty() {
			t.Fatalf("expected dirty after edit")
		}

		// Undo → content returns to "hello" → dirty should be false
		m, _ = m.applyUndo()
		m = m.syncDisplay()

		if m.Content() != "hello" {
			t.Fatalf("expected content 'hello' after undo, got %q", m.Content())
		}
		if m.IsDirty() {
			t.Fatalf("expected clean after undo to saved state")
		}
	})

	// Gate 8: Edit/save/edit/undo → clean; redo → dirty
	t.Run("gate8/save-boundary-dirty-tracking", func(t *testing.T) {
		m := newTestEditor("hello")
		m.cursors = cursor.NewCursorSet(5)
		m.savedContentHash = hashContent("hello")
		m.dirty = false
		clk := editortest.NewClock()
		m.history = history.New(clk.Now)

		// Edit (makes dirty)
		clk = clk.Advance(500 * time.Millisecond)
		m = applyCmd(m, "edit.insert-character", a("A"), clk.Now())
		if !m.IsDirty() {
			t.Fatalf("expected dirty after first edit")
		}

		// Simulate save: update savedContentHash to current content
		m.savedContentHash = hashContent(m.Content())
		m.dirty = false

		// Another edit (makes dirty again)
		clk = clk.Advance(500 * time.Millisecond)
		m = applyCmd(m, "edit.insert-character", a("B"), clk.Now())
		if !m.IsDirty() {
			t.Fatalf("expected dirty after second edit")
		}

		// Undo back to saved state ("helloA")
		m, _ = m.applyUndo()
		m = m.syncDisplay()

		if m.Content() != "helloA" {
			t.Fatalf("expected 'helloA' after undo, got %q", m.Content())
		}
		if m.IsDirty() {
			t.Fatalf("expected clean after undo to saved state")
		}

		// Redo → content goes back to "helloAB" → dirty
		m, _ = m.applyRedo()
		m = m.syncDisplay()

		if m.Content() != "helloAB" {
			t.Fatalf("expected 'helloAB' after redo, got %q", m.Content())
		}
		if !m.IsDirty() {
			t.Fatalf("expected dirty after redo past save point")
		}
	})
}
