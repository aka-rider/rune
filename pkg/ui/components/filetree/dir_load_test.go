package filetree

import (
	"testing"

	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"
)

func newDirLoadTest(entries []Entry, cursor int) Model {
	keys := keymap.Default()
	st := styles.Default()
	m := New(keys, st)
	m = m.SetSize(20, 10)
	m.entries = entries
	m.cursor = cursor
	return m
}

func TestDirLoadedResetsCursor(t *testing.T) {
	m := newDirLoadTest([]Entry{
		{Name: "a.txt", Path: "a.txt", IsDir: false},
		{Name: "b.txt", Path: "b.txt", IsDir: false},
		{Name: "c.txt", Path: "c.txt", IsDir: false},
	}, 2) // pointing at "c.txt"

	// Navigate into a new directory — cursor should reset to 0.
	m, _ = m.Update(DirLoadedMsg{
		Root: "/new/dir",
		Entries: []Entry{
			{Name: "x.txt", Path: "/new/dir/x.txt", IsDir: false},
			{Name: "y.txt", Path: "/new/dir/y.txt", IsDir: false},
		},
	})

	if m.cursor != 0 {
		t.Errorf("cursor = %d, want 0 (navigation should reset)", m.cursor)
	}
	if m.root != "/new/dir" {
		t.Errorf("root = %q, want %q", m.root, "/new/dir")
	}
}

func TestDirReloadedPreservesCursorByName(t *testing.T) {
	m := newDirLoadTest([]Entry{
		{Name: "a.txt", Path: "a.txt", IsDir: false},
		{Name: "b.txt", Path: "b.txt", IsDir: false},
		{Name: "c.txt", Path: "c.txt", IsDir: false},
	}, 1) // pointing at "b.txt"

	// Disk-triggered reload — same directory, entries may have changed.
	// "b.txt" should still be selected.
	m, _ = m.Update(DirReloadedMsg{
		Root: ".",
		Entries: []Entry{
			{Name: "a.txt", Path: "a.txt", IsDir: false},
			{Name: "b.txt", Path: "b.txt", IsDir: false},
			{Name: "d.txt", Path: "d.txt", IsDir: false}, // new file added
		},
	})

	if m.cursor != 1 {
		t.Errorf("cursor = %d, want 1 (name 'b.txt' should be preserved)", m.cursor)
	}
}

func TestDirReloadedPreservesCursorByIndexFallback(t *testing.T) {
	m := newDirLoadTest([]Entry{
		{Name: "a.txt", Path: "a.txt", IsDir: false},
		{Name: "b.txt", Path: "b.txt", IsDir: false},
	}, 1) // pointing at "b.txt"

	// Reload with completely different entries — name not found, fall back to index.
	m, _ = m.Update(DirReloadedMsg{
		Root: ".",
		Entries: []Entry{
			{Name: "x.txt", Path: "x.txt", IsDir: false},
			{Name: "y.txt", Path: "y.txt", IsDir: false},
			{Name: "z.txt", Path: "z.txt", IsDir: false},
		},
	})

	if m.cursor != 1 {
		t.Errorf("cursor = %d, want 1 (index fallback)", m.cursor)
	}
}

func TestDirReloadedClampsCursorWhenEntriesShrink(t *testing.T) {
	m := newDirLoadTest([]Entry{
		{Name: "a.txt", Path: "a.txt", IsDir: false},
		{Name: "b.txt", Path: "b.txt", IsDir: false},
		{Name: "c.txt", Path: "c.txt", IsDir: false},
	}, 2) // pointing at "c.txt"

	// Reload with fewer entries — cursor should clamp to last entry.
	m, _ = m.Update(DirReloadedMsg{
		Root: ".",
		Entries: []Entry{
			{Name: "a.txt", Path: "a.txt", IsDir: false},
		},
	})

	if m.cursor != 0 {
		t.Errorf("cursor = %d, want 0 (clamped to last entry)", m.cursor)
	}
}

func TestDirReloadedEmptyDirectory(t *testing.T) {
	m := newDirLoadTest([]Entry{
		{Name: "a.txt", Path: "a.txt", IsDir: false},
	}, 0)

	// All files deleted — cursor should be 0.
	m, _ = m.Update(DirReloadedMsg{
		Root:    ".",
		Entries: []Entry{},
	})

	if m.cursor != 0 {
		t.Errorf("cursor = %d, want 0 (empty directory)", m.cursor)
	}
}
