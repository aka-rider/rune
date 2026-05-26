package editor

import (
	"testing"
	"time"

	"rune/internal/editortest"
	"rune/pkg/command"
	"rune/pkg/editor/buffer"
	"rune/pkg/editor/cursor"
	"rune/pkg/editor/keybind"
	"rune/pkg/terminal"
	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"

	tea "charm.land/bubbletea/v2"
)

func newTestEditorForClipboard(content string) Model {
	keys := keymap.Default()
	st := styles.Default()

	builder := command.NewBuilder()
	builder, _ = registerClipboardCommands(builder)
	builder, _ = registerEditCommands(builder)
	reg := builder.Build()
	res, _ := keybind.NewResolver(nil)

	m := New(keys, st, reg, res, terminal.TermCaps{})
	m = m.SetSize(80, 24)
	m = m.SetFocused(true)
	m = m.SetContent("test.txt", []byte(content))

	return m
}

// setEditorState sets up buffer and cursors from notation.
func setEditorState(m Model, notation string) Model {
	st, err := editortest.ParseState(notation)
	if err != nil {
		panic("bad notation: " + err.Error())
	}
	m.buf = buffer.New(st.Content)
	var cList []cursor.Cursor
	for i, c := range st.Cursors {
		cList = append(cList, cursor.Cursor{Position: c.Position, Anchor: c.Anchor, ID: i})
	}
	m.cursors = cursor.NewCursorSetFrom(cList)
	m = m.syncDisplay()
	return m
}

// getEditorState returns the current editor state as notation.
func getEditorState(m Model) string {
	var outCursors []editortest.CursorState
	for _, c := range m.cursors.All() {
		outCursors = append(outCursors, editortest.CursorState{Position: c.Position, Anchor: c.Anchor})
	}
	return editortest.FormatState(editortest.TestState{
		Content: m.buf.Content(),
		Cursors: outCursors,
	})
}

// executePaste simulates paste: command → tea.ClipboardMsg with given text.
func executePaste(m Model, text string) Model {
	ctx := command.CommandContext{
		Buffer:  m.buf,
		Cursors: m.cursors,
	}
	res := m.registry.Execute("clipboard.paste", ctx)
	if res.Err != nil {
		panic("clipboard.paste error: " + res.Err.Error())
	}

	// Phase 1: dispatch operation (returns read cmd)
	m, cmd := m.dispatchOperation(res, "clipboard.paste", time.Now())
	if cmd == nil {
		panic("clipboard.paste returned nil Cmd")
	}

	// Phase 2: simulate the clipboard response arriving
	m, _ = m.Update(tea.ClipboardMsg{Content: text})
	return m
}

// executeCopy performs the copy command and returns the Cmd (tea.SetClipboard).
func executeCopy(m Model) (Model, tea.Cmd) {
	ctx := command.CommandContext{
		Buffer:  m.buf,
		Cursors: m.cursors,
	}
	res := m.registry.Execute("clipboard.copy", ctx)
	if res.Err != nil {
		panic("clipboard.copy error: " + res.Err.Error())
	}
	return m.dispatchOperation(res, "clipboard.copy", time.Now())
}

// executeCut performs the cut command and returns the Cmd (tea.SetClipboard).
func executeCut(m Model) (Model, tea.Cmd) {
	ctx := command.CommandContext{
		Buffer:  m.buf,
		Cursors: m.cursors,
	}
	res := m.registry.Execute("clipboard.cut", ctx)
	if res.Err != nil {
		panic("clipboard.cut error: " + res.Err.Error())
	}
	return m.dispatchOperation(res, "clipboard.cut", time.Now())
}

func TestSpec_Clipboard(t *testing.T) {
	t.Run("paste/basic", func(t *testing.T) {
		m := newTestEditorForClipboard("hello")
		m = setEditorState(m, "hel|lo")

		m = executePaste(m, "XY")

		got := getEditorState(m)
		want := "helXY|lo"
		if got != want {
			t.Fatalf("paste/basic:\n  got:  %q\n  want: %q", got, want)
		}
	})

	t.Run("paste/replace-sel", func(t *testing.T) {
		m := newTestEditorForClipboard("hello")
		m = setEditorState(m, "h[ell]o")

		m = executePaste(m, "XY")

		got := getEditorState(m)
		want := "hXY|o"
		if got != want {
			t.Fatalf("paste/replace-sel:\n  got:  %q\n  want: %q", got, want)
		}
	})

	t.Run("paste/distribute", func(t *testing.T) {
		// Gate 1: N lines into N cursors → distribute
		m := newTestEditorForClipboard("aa\nbb")
		m = setEditorState(m, "a|a\nb|b")

		m = executePaste(m, "X\nY")

		got := getEditorState(m)
		want := "aX|a\nbY|b"
		if got != want {
			t.Fatalf("paste/distribute:\n  got:  %q\n  want: %q", got, want)
		}
	})

	t.Run("paste/no-distribute-mismatch", func(t *testing.T) {
		// Gate 2: N lines into M cursors (N≠M) → full text at each cursor
		// 3 lines, 2 cursors:
		m := newTestEditorForClipboard("aa\nbb")
		m = setEditorState(m, "a|a\nb|b") // 2 cursors, 3 lines → full paste at each

		m = executePaste(m, "X\nY\nZ")

		got := getEditorState(m)
		// Full "X\nY\nZ" at each cursor position
		want := "aX\nY\nZ|a\nbX\nY\nZ|b"
		if got != want {
			t.Fatalf("paste/no-distribute-mismatch:\n  got:  %q\n  want: %q", got, want)
		}
	})

	t.Run("copy/no-sel", func(t *testing.T) {
		// Gate 3: Copy with no selection = entire line including trailing \n
		m := newTestEditorForClipboard("hello\nworld")
		m = setEditorState(m, "hel|lo\nworld")

		m, cmd := executeCopy(m)

		// Cmd must be non-nil (tea.SetClipboard)
		if cmd == nil {
			t.Fatal("copy/no-sel: returned nil Cmd")
		}

		// Editor state should be unchanged
		got := getEditorState(m)
		want := "hel|lo\nworld"
		if got != want {
			t.Fatalf("copy/no-sel: state changed to %q, want %q", got, want)
		}
	})

	t.Run("copy/with-sel", func(t *testing.T) {
		m := newTestEditorForClipboard("hello")
		m = setEditorState(m, "h[ell]o")

		_, cmd := executeCopy(m)

		if cmd == nil {
			t.Fatal("copy/with-sel: returned nil Cmd")
		}
	})

	t.Run("cut/with-sel", func(t *testing.T) {
		// Gate 4: Cut with selection: content deleted AND clipboard cmd returned
		m := newTestEditorForClipboard("hello")
		m = setEditorState(m, "h[ell]o")

		m, cmd := executeCut(m)

		got := getEditorState(m)
		want := "h|o"
		if got != want {
			t.Fatalf("cut/with-sel: state got %q, want %q", got, want)
		}
		if cmd == nil {
			t.Fatal("cut/with-sel: returned nil Cmd (clipboard write lost)")
		}
	})

	t.Run("cut/no-sel", func(t *testing.T) {
		m := newTestEditorForClipboard("hello\nworld")
		m = setEditorState(m, "hel|lo\nworld")

		m, cmd := executeCut(m)

		got := getEditorState(m)
		want := "|world"
		if got != want {
			t.Fatalf("cut/no-sel: state got %q, want %q", got, want)
		}
		if cmd == nil {
			t.Fatal("cut/no-sel: returned nil Cmd (clipboard write lost)")
		}
	})

	t.Run("paste/edit-kind-is-paste", func(t *testing.T) {
		// Gate 5: Paste is EditPaste kind: never coalesces
		m := newTestEditorForClipboard("ab")
		m = setEditorState(m, "a|b")

		// Do a first paste
		m = executePaste(m, "X")

		// Do a second paste immediately (same tick)
		m = executePaste(m, "Y")

		// Two separate paste operations should create two history entries
		// Undo the last paste
		m, _ = m.applyUndo()
		m = m.syncDisplay()
		got := getEditorState(m)
		want := "aX|b"
		if got != want {
			t.Fatalf("paste/edit-kind: after undo got %q, want %q (paste was coalesced)", got, want)
		}
	})

	t.Run("paste/trailing-newline-distributes", func(t *testing.T) {
		// Gate 6: Clipboard text with trailing newline preserves distribution semantics
		// "X\nY\n" with 2 cursors should distribute "X" and "Y" (trailing newline stripped)
		m := newTestEditorForClipboard("aa\nbb")
		m = setEditorState(m, "a|a\nb|b")

		m = executePaste(m, "X\nY\n")

		got := getEditorState(m)
		want := "aX|a\nbY|b"
		if got != want {
			t.Fatalf("paste/trailing-newline-distributes:\n  got:  %q\n  want: %q", got, want)
		}
	})

	t.Run("copy/returns-non-nil-cmd-production-path", func(t *testing.T) {
		// Regression guard: editor constructed via New() (production path)
		// must return non-nil Cmd from clipboard commands — no ClipboardPort needed.
		m := newTestEditorForClipboard("hello world")
		m = setEditorState(m, "h[ello] world")

		_, cmd := executeCopy(m)
		if cmd == nil {
			t.Fatal("production-path copy returned nil Cmd — clipboard broken")
		}
	})

	t.Run("paste/returns-non-nil-cmd-production-path", func(t *testing.T) {
		// Regression guard: paste command always returns a non-nil read Cmd.
		m := newTestEditorForClipboard("hello")
		m = setEditorState(m, "hel|lo")

		ctx := command.CommandContext{
			Buffer:  m.buf,
			Cursors: m.cursors,
		}
		res := m.registry.Execute("clipboard.paste", ctx)
		if res.Err != nil {
			t.Fatalf("clipboard.paste error: %v", res.Err)
		}
		_, cmd := m.dispatchOperation(res, "clipboard.paste", time.Now())
		if cmd == nil {
			t.Fatal("production-path paste returned nil Cmd — clipboard broken")
		}
	})
}
