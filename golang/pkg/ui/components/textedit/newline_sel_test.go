package textedit_test

import (
	"testing"

	"rune/pkg/command"
	"rune/pkg/editor/buffer"
	"rune/pkg/editor/cursor"
	"rune/pkg/ui/components/textedit"
	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"
)

func buildTextEditRegistry(t *testing.T) command.Registry {
	t.Helper()
	b := command.NewBuilder()
	b, err := textedit.RegisterCommands(b)
	if err != nil {
		t.Fatalf("RegisterCommands: %v", err)
	}
	return b.Build()
}

// execOnBuf runs a registered edit command against a buffer+cursor and returns
// the resulting content. The registry skips the When-condition check, so
// "editorFocused" is not required here.
func execOnBuf(t *testing.T, reg command.Registry, name, content string, cur cursor.Cursor) string {
	t.Helper()
	buf := buffer.New(content)
	cursors := cursor.NewCursorSetFrom([]cursor.Cursor{cur})
	ctx := command.CommandContext{Buffer: buf, Cursors: cursors}
	result := reg.Execute(name, ctx)
	if result.Err != nil {
		t.Fatalf("Execute %q: %v", name, result.Err)
	}
	if len(result.Operation.Edits) == 0 {
		return content
	}
	sorted := buffer.CloneAndSortEditsDescending(result.Operation.Edits)
	newBuf, _, err := buf.ApplyEdits(sorted)
	if err != nil {
		t.Fatalf("ApplyEdits: %v", err)
	}
	return newBuf.Content()
}

// TestSelectionDeleteDoesNotConsumeNewline verifies that when the selection
// boundary (SelectionEnd) lands exactly on '\n', no delete or insert command
// consumes that '\n'. This is the regression test for the reversed-selection
// anchor-advance bug: selectionEndInclusive used to advance past '\n', merging
// the next line into the deletion.
func TestSelectionDeleteDoesNotConsumeNewline(t *testing.T) {
	reg := buildTextEditRegistry(t)

	// "hello\nworld": '\n' at byte 5.
	// Reversed selection: cursor=0, anchor=5 (End → Shift+Home).
	reversed := cursor.Cursor{Position: 0, Anchor: 5, ID: 1}
	// Forward selection: cursor=5, anchor=0 (Shift+End from column 0).
	forward := cursor.Cursor{Position: 5, Anchor: 0, ID: 1}

	cases := []struct {
		name    string
		content string
		cur     cursor.Cursor
		cmd     string
		want    string
	}{
		{
			name:    "reversed anchor at newline / delete-right preserves newline",
			content: "hello\nworld",
			cur:     reversed,
			cmd:     "edit.delete-right",
			want:    "\nworld",
		},
		{
			name:    "reversed anchor at newline / delete-left preserves newline",
			content: "hello\nworld",
			cur:     reversed,
			cmd:     "edit.delete-left",
			want:    "\nworld",
		},
		{
			name:    "forward selection ending at newline / delete-right preserves newline",
			content: "hello\nworld",
			cur:     forward,
			cmd:     "edit.delete-right",
			want:    "\nworld",
		},
		{
			name: "reversed anchor NOT at newline still consumes anchor char",
			// "hello": cursor=0, anchor=3 ('l'). selectionEndInclusive advances
			// past 'l' → bytes [0,4) deleted → "o" remains.
			content: "hello",
			cur:     cursor.Cursor{Position: 0, Anchor: 3, ID: 1},
			cmd:     "edit.delete-right",
			want:    "o",
		},
		{
			name:    "reversed anchor at newline in middle of file",
			content: "ab\ncd\nef",
			cur:     cursor.Cursor{Position: 0, Anchor: 2, ID: 1}, // anchor at first '\n'
			cmd:     "edit.delete-right",
			want:    "\ncd\nef",
		},
	}

	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			got := execOnBuf(t, reg, tc.cmd, tc.content, tc.cur)
			if got != tc.want {
				t.Errorf("content = %q, want %q", got, tc.want)
			}
		})
	}
}

// TestSelectionsIntervalExcludesNewlineAtBoundary verifies that Selections()
// does not extend the highlight interval past '\n' when a reversed selection's
// anchor sits on '\n'. The interval must match the deletion range.
func TestSelectionsIntervalExcludesNewlineAtBoundary(t *testing.T) {
	m := textedit.New(keymap.Default(), styles.Default())
	m = m.SetContent("hello\nworld")
	// Reversed selection: cursor=0, anchor=5 ('\n').
	m = m.SetCursors([]cursor.Cursor{{Position: 0, Anchor: 5, ID: 1}})

	sels := m.Selections()
	if len(sels) != 1 {
		t.Fatalf("Selections() len = %d, want 1", len(sels))
	}
	got := sels[0]
	if got.Start != 0 || got.End != 5 {
		t.Errorf("Selections()[0] = {%d,%d}, want {0,5}", got.Start, got.End)
	}
}
