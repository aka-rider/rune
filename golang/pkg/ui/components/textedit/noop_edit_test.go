package textedit

import (
	"testing"

	"rune/pkg/command"
	"rune/pkg/editor/buffer"
	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"
)

// TestZeroWidthEditBatchIsANoOp is the parity fix for the Rust port's WP1: a
// zero-width, insert-nothing edit (Start == End, Insert == "") reaches
// applyOperation whenever a command derives its edit from an empty
// selection-or-line (e.g. clipboard.cut with no selection). It is a legal
// no-op at the buffer layer — ApplyEdits accepts it correctly — but
// committing it anyway used to still bump rev and append to pendingEdits
// for an operation that changed nothing, marking a clean document dirty
// once the workspace layer reads rev.
//
// D5: package textedit (internal), not textedit_test — applyOperation is
// unexported, and driving it directly through a hand-built command.Result
// is the only way to exercise the actual chokepoint the fix lives in,
// rather than the buffer-level ApplyEdits call the registry-only tests in
// this package use.
func TestZeroWidthEditBatchIsANoOp(t *testing.T) {
	t.Run("empty buffer", func(t *testing.T) {
		m := New(keymap.Default(), styles.Default())
		m = m.SetRect(Rect{W: 80, H: 24})
		m = m.SetContent("")
		revBefore := m.Revision()
		_, pendingBefore := m.DrainEdits()

		m = m.applyOperation(command.Result{
			Operation: command.Operation{
				Kind:    command.OperationEditBuffer,
				Edits:   []buffer.Edit{{Start: 0, End: 0, Insert: ""}},
				Cursors: m.cursors,
			},
		})

		if got := m.Revision(); got != revBefore {
			t.Errorf("rev = %d, want unchanged %d", got, revBefore)
		}
		_, pendingAfter := m.DrainEdits()
		if len(pendingAfter) != len(pendingBefore) {
			t.Errorf("pendingEdits len = %d, want unchanged %d", len(pendingAfter), len(pendingBefore))
		}
		if got := m.Content(); got != "" {
			t.Errorf("Content() = %q, want unchanged %q", got, "")
		}
	})

	t.Run("empty last line", func(t *testing.T) {
		m := New(keymap.Default(), styles.Default())
		m = m.SetRect(Rect{W: 80, H: 24})
		m = m.SetContent("a\n")
		revBefore := m.Revision()
		_, pendingBefore := m.DrainEdits()

		// Byte 2 is EOF on "a\n" — the empty last line has no trailing '\n'
		// past it to include, so the derived range is zero-width.
		m = m.applyOperation(command.Result{
			Operation: command.Operation{
				Kind:    command.OperationEditBuffer,
				Edits:   []buffer.Edit{{Start: 2, End: 2, Insert: ""}},
				Cursors: m.cursors,
			},
		})

		if got := m.Revision(); got != revBefore {
			t.Errorf("rev = %d, want unchanged %d", got, revBefore)
		}
		_, pendingAfter := m.DrainEdits()
		if len(pendingAfter) != len(pendingBefore) {
			t.Errorf("pendingEdits len = %d, want unchanged %d", len(pendingAfter), len(pendingBefore))
		}
		if got := m.Content(); got != "a\n" {
			t.Errorf("Content() = %q, want unchanged %q", got, "a\n")
		}
	})
}
