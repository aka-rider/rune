package editor

import (
	"testing"
	"time"

	"rune/internal/editortest"
	"rune/pkg/command"
	"rune/pkg/editor/buffer"
	"rune/pkg/editor/cursor"
	"rune/pkg/editor/keybind"
	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"

	tea "charm.land/bubbletea/v2"
)

// mockClipboard is a simple in-memory clipboard for testing.
type mockClipboard struct {
	content string
}

func (c *mockClipboard) ReadText() (string, error)  { return c.content, nil }
func (c *mockClipboard) WriteText(text string) error { c.content = text; return nil }

func newTestEditorWithClipboard(content string, clipText string) (Model, *mockClipboard) {
	keys := keymap.Default()
	st := styles.Default()

	builder := command.NewBuilder()
	builder, _ = registerClipboardCommands(builder)
	builder, _ = registerEditCommands(builder)
	reg := builder.Build()
	res, _ := keybind.NewResolver(nil)

	m := New(keys, st, reg, res)
	m = m.SetSize(80, 24)
	m = m.SetFocused(true)
	m = m.SetContent("test.txt", []byte(content))

	clip := &mockClipboard{content: clipText}
	m = m.SetClipboard(ClipboardPort{
		ReadText:  clip.ReadText,
		WriteText: clip.WriteText,
	})

	return m, clip
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

// executePaste performs the two-phase paste: command → ClipboardContentMsg.
func executePaste(m Model) (Model, tea.Cmd) {
	ctx := command.CommandContext{
		Buffer:  m.buf,
		Cursors: m.cursors,
	}
	res := m.registry.Execute("clipboard.paste", ctx)
	if res.Err != nil {
		panic("clipboard.paste error: " + res.Err.Error())
	}

	// Phase 1: dispatch operation (builds read cmd)
	m, cmd := m.dispatchOperation(res, "clipboard.paste", time.Now())

	// Phase 2: execute the cmd to get ClipboardContentMsg, feed it back
	if cmd != nil {
		msg := cmd()
		if clipMsg, ok := msg.(ClipboardContentMsg); ok {
			m, _ = m.Update(clipMsg)
		}
	}

	return m, nil
}

// executeCopy performs the copy command and returns the cmd for clipboard write.
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

// executeCut performs the cut command.
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
		m, clip := newTestEditorWithClipboard("hello", "XY")
		_ = clip
		m = setEditorState(m, "hel|lo")

		m, _ = executePaste(m)

		got := getEditorState(m)
		want := "helXY|lo"
		if got != want {
			t.Fatalf("paste/basic:\n  got:  %q\n  want: %q", got, want)
		}
	})

	t.Run("paste/replace-sel", func(t *testing.T) {
		m, clip := newTestEditorWithClipboard("hello", "XY")
		_ = clip
		m = setEditorState(m, "h[ell]o")

		m, _ = executePaste(m)

		got := getEditorState(m)
		want := "hXY|o"
		if got != want {
			t.Fatalf("paste/replace-sel:\n  got:  %q\n  want: %q", got, want)
		}
	})

	t.Run("paste/distribute", func(t *testing.T) {
		// Gate 1: N lines into N cursors → distribute
		m, clip := newTestEditorWithClipboard("aa\nbb", "X\nY")
		_ = clip
		m = setEditorState(m, "a|a\nb|b")

		m, _ = executePaste(m)

		got := getEditorState(m)
		want := "aX|a\nbY|b"
		if got != want {
			t.Fatalf("paste/distribute:\n  got:  %q\n  want: %q", got, want)
		}
	})

	t.Run("paste/no-distribute-mismatch", func(t *testing.T) {
		// Gate 2: N lines into M cursors (N≠M) → full text at each cursor
		m, clip := newTestEditorWithClipboard("aa\nbb\ncc", "X\nY\nZ")
		_ = clip
		m = setEditorState(m, "a|a\nb|b\nc|c") // 3 cursors, 3 lines → distribute
		// Actually for N==M, it distributes. Let's test N≠M.
		// 3 lines, 2 cursors:
		m, clip = newTestEditorWithClipboard("aa\nbb", "X\nY\nZ")
		_ = clip
		m = setEditorState(m, "a|a\nb|b") // 2 cursors, 3 lines → full paste at each

		m, _ = executePaste(m)

		got := getEditorState(m)
		// Full "X\nY\nZ" at each cursor position
		// First cursor at offset 1 in "aa": "a" + "X\nY\nZ" + "a\n..."
		// Second cursor at offset 4 in "aa\nbb" = offset 1 in "bb"
		// After paste at both: "aX\nY\nZ|a\nbX\nY\nZ|b"
		want := "aX\nY\nZ|a\nbX\nY\nZ|b"
		if got != want {
			t.Fatalf("paste/no-distribute-mismatch:\n  got:  %q\n  want: %q", got, want)
		}
	})

	t.Run("copy/no-sel", func(t *testing.T) {
		// Gate 3: Copy with no selection = entire line including trailing \n
		m, clip := newTestEditorWithClipboard("hello\nworld", "")
		m = setEditorState(m, "hel|lo\nworld")

		m, cmd := executeCopy(m)

		// Execute the write cmd
		if cmd != nil {
			cmd()
		}

		if clip.content != "hello\n" {
			t.Fatalf("copy/no-sel: clipboard got %q, want %q", clip.content, "hello\n")
		}

		// Editor state should be unchanged
		got := getEditorState(m)
		want := "hel|lo\nworld"
		if got != want {
			t.Fatalf("copy/no-sel: state changed to %q, want %q", got, want)
		}
	})

	t.Run("copy/with-sel", func(t *testing.T) {
		m, clip := newTestEditorWithClipboard("hello", "")
		m = setEditorState(m, "h[ell]o")

		_, cmd := executeCopy(m)

		if cmd != nil {
			cmd()
		}

		if clip.content != "ell" {
			t.Fatalf("copy/with-sel: clipboard got %q, want %q", clip.content, "ell")
		}
	})

	t.Run("cut/with-sel", func(t *testing.T) {
		// Gate 4: Cut with selection: content deleted AND clipboard has selected text
		m, clip := newTestEditorWithClipboard("hello", "")
		m = setEditorState(m, "h[ell]o")

		m, cmd := executeCut(m)

		// Execute write cmd
		if cmd != nil {
			cmd()
		}

		got := getEditorState(m)
		want := "h|o"
		if got != want {
			t.Fatalf("cut/with-sel: state got %q, want %q", got, want)
		}
		if clip.content != "ell" {
			t.Fatalf("cut/with-sel: clipboard got %q, want %q", clip.content, "ell")
		}
	})

	t.Run("cut/no-sel", func(t *testing.T) {
		m, clip := newTestEditorWithClipboard("hello\nworld", "")
		m = setEditorState(m, "hel|lo\nworld")

		m, cmd := executeCut(m)

		if cmd != nil {
			cmd()
		}

		got := getEditorState(m)
		want := "|world"
		if got != want {
			t.Fatalf("cut/no-sel: state got %q, want %q", got, want)
		}
		if clip.content != "hello\n" {
			t.Fatalf("cut/no-sel: clipboard got %q, want %q", clip.content, "hello\n")
		}
	})

	t.Run("paste/edit-kind-is-paste", func(t *testing.T) {
		// Gate 5: Paste is EditPaste kind: never coalesces
		m, _ := newTestEditorWithClipboard("ab", "X")
		m = setEditorState(m, "a|b")

		// Do a first paste
		m, _ = executePaste(m)

		// Record history state
		histBefore := m.history

		// Do a second paste immediately (same tick)
		m.clipboard = ClipboardPort{
			ReadText:  func() (string, error) { return "Y", nil },
			WriteText: func(string) error { return nil },
		}
		m, _ = executePaste(m)

		// History should have a NEW group (not coalesced with previous)
		if !histBefore.CanUndo() {
			t.Fatal("expected history to have undo after first paste")
		}
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
		m, clip := newTestEditorWithClipboard("aa\nbb", "X\nY\n")
		_ = clip
		m = setEditorState(m, "a|a\nb|b")

		m, _ = executePaste(m)

		got := getEditorState(m)
		want := "aX|a\nbY|b"
		if got != want {
			t.Fatalf("paste/trailing-newline-distributes:\n  got:  %q\n  want: %q", got, want)
		}
	})
}
