package filetree

import (
	"testing"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"
)

func newFocusedModel(entries []Entry, cursor int) Model {
	keys := keymap.Default()
	st := styles.Default()
	m := New(keys, st)
	m = m.SetSize(20, 10)
	m = m.SetFocused(true)
	m.entries = entries
	m.nav.Cursor = cursor
	return m
}

// ─── RemoveEntry ────────────────────────────────────────────────────────────

func TestRemoveEntryFiltersPath(t *testing.T) {
	m := newFocusedModel([]Entry{
		{Name: "a.md", Path: "/w/a.md"},
		{Name: "b.md", Path: "/w/b.md"},
		{Name: "c.md", Path: "/w/c.md"},
	}, 1)

	m = m.RemoveEntry("/w/b.md")

	if m.Len() != 2 {
		t.Fatalf("Len = %d, want 2", m.Len())
	}
	for _, e := range m.entries {
		if e.Path == "/w/b.md" {
			t.Fatal("removed entry still present")
		}
	}
}

func TestRemoveEntryClampsOOBCursor(t *testing.T) {
	m := newFocusedModel([]Entry{
		{Name: "a.md", Path: "/w/a.md"},
		{Name: "b.md", Path: "/w/b.md"},
	}, 1) // cursor on b.md

	m = m.RemoveEntry("/w/b.md")

	if m.nav.Cursor != 0 {
		t.Errorf("cursor = %d, want 0 after removing the last entry", m.nav.Cursor)
	}
}

func TestRemoveEntryUnknownPathIsNoop(t *testing.T) {
	m := newFocusedModel([]Entry{
		{Name: "a.md", Path: "/w/a.md"},
	}, 0)

	before := m.Len()
	m = m.RemoveEntry("/w/nonexistent.md")

	if m.Len() != before {
		t.Errorf("Len changed from %d to %d — noop expected for unknown path", before, m.Len())
	}
}

// ─── Key routing ────────────────────────────────────────────────────────────

func trashKey() tea.KeyPressMsg {
	return tea.KeyPressMsg{Code: tea.KeyBackspace, Mod: tea.ModSuper}
}

func deleteKey() tea.KeyPressMsg {
	return tea.KeyPressMsg{Code: tea.KeyDelete}
}

func TestTrashKeyFocusedEmitsFileDeleteRequestedMsg(t *testing.T) {
	m := newFocusedModel([]Entry{
		{Name: "notes.md", Path: "/w/notes.md"},
	}, 0)

	_, cmd := m.Update(trashKey())
	if cmd == nil {
		t.Fatal("expected a Cmd, got nil")
	}
	result := cmd()
	got, ok := result.(FileDeleteRequestedMsg)
	if !ok {
		t.Fatalf("Cmd() returned %T, want FileDeleteRequestedMsg", result)
	}
	if got.Path != "/w/notes.md" {
		t.Errorf("Path = %q, want %q", got.Path, "/w/notes.md")
	}
}

func TestTrashKeyUnfocusedIgnored(t *testing.T) {
	keys := keymap.Default()
	st := styles.Default()
	m := New(keys, st)
	m = m.SetSize(20, 10)
	m = m.SetFocused(false)
	m.entries = []Entry{{Name: "notes.md", Path: "/w/notes.md"}}

	_, cmd := m.Update(trashKey())
	if cmd != nil {
		t.Fatal("unfocused filetree must not emit a Cmd on trash key")
	}
}

func TestTrashKeyOnDotDotIgnored(t *testing.T) {
	m := newFocusedModel([]Entry{
		{Name: "..", Path: "/parent", IsDir: true},
		{Name: "notes.md", Path: "/w/notes.md"},
	}, 0) // cursor on ".."

	_, cmd := m.Update(trashKey())
	if cmd != nil {
		t.Fatal("trash key on '..' must not emit a Cmd")
	}
}

func TestTrashKeyEmptyEntriesIgnored(t *testing.T) {
	m := newFocusedModel(nil, 0)

	_, cmd := m.Update(trashKey())
	if cmd != nil {
		t.Fatal("trash key on empty filetree must not emit a Cmd")
	}
}

func TestDeleteKeyFocusedEmitsFileDeleteRequestedMsg(t *testing.T) {
	m := newFocusedModel([]Entry{
		{Name: "notes.md", Path: "/w/notes.md"},
	}, 0)

	_, cmd := m.Update(deleteKey())
	if cmd == nil {
		t.Fatal("expected a Cmd for ⌦ alias, got nil")
	}
	result := cmd()
	got, ok := result.(FileDeleteRequestedMsg)
	if !ok {
		t.Fatalf("Cmd() returned %T, want FileDeleteRequestedMsg", result)
	}
	if got.Path != "/w/notes.md" {
		t.Errorf("Path = %q, want %q", got.Path, "/w/notes.md")
	}
}

func TestDeleteKeyUnfocusedIgnored(t *testing.T) {
	keys := keymap.Default()
	st := styles.Default()
	m := New(keys, st)
	m = m.SetSize(20, 10)
	m = m.SetFocused(false)
	m.entries = []Entry{{Name: "notes.md", Path: "/w/notes.md"}}

	_, cmd := m.Update(deleteKey())
	if cmd != nil {
		t.Fatal("unfocused filetree must not emit a Cmd on ⌦ key")
	}
}
