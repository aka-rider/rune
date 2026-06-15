package opentabs

import (
	"strings"
	"testing"

	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"
)

func TestDirtyFlagPosition(t *testing.T) {
	m := New(keymap.Default(), styles.Default())
	m = m.SetSize(60, 10)
	m = m.OpenFile("/notes/tickets.txt")

	// Clean: no 'x' appears before the filename in the rendered view.
	view := m.View()
	nameIdx := strings.Index(view, "tickets.txt")
	if nameIdx < 0 {
		t.Fatalf("expected 'tickets.txt' in clean view, got:\n%s", view)
	}
	if strings.Contains(view[:nameIdx], "x") {
		t.Errorf("clean tab must not contain 'x' before the filename, got:\n%s", view)
	}

	// Dirty: 'x' must appear before the filename (in the number-prefix slot).
	m = m.MarkDirty("/notes/tickets.txt")
	view = m.View()
	nameIdx = strings.Index(view, "tickets.txt")
	if nameIdx < 0 {
		t.Fatalf("expected 'tickets.txt' in dirty view, got:\n%s", view)
	}
	if !strings.Contains(view[:nameIdx], "x") {
		t.Errorf("dirty tab must have 'x' before the filename, got:\n%s", view)
	}
	// Dirty marker must NOT appear after the filename.
	afterName := view[nameIdx+len("tickets.txt"):]
	if strings.Contains(afterName, "x") || strings.Contains(afterName, "●") {
		t.Errorf("dirty marker must not appear after the filename, got:\n%s", view)
	}
}
