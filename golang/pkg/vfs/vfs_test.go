package vfs

import (
	"errors"
	"io/fs"
	"os"
	"path/filepath"
	"testing"
)

// TestDisk_WriteReadRoundTrip: Disk.WriteFile is byte-faithful (in-place
// create/write/fsync — no temp, no residue by construction, since atomicity
// of a published destination is now Exchange/RenameExcl's job, not
// WriteFile's), and Stat/FileID report a real inode (§1.4.1/§1.4.5/§1.4.6).
func TestDisk_WriteReadRoundTrip(t *testing.T) {
	dir := t.TempDir()
	p := filepath.Join(dir, "note.md")
	content := []byte("# Title\r\nno trailing newline") // CRLF + no \n: must round-trip verbatim

	if err := (Disk{}).WriteFile(p, content, 0o644); err != nil {
		t.Fatalf("WriteFile: %v", err)
	}
	got, err := (Disk{}).ReadFile(p)
	if err != nil {
		t.Fatalf("ReadFile: %v", err)
	}
	if string(got) != string(content) {
		t.Fatalf("round-trip mismatch: got %q want %q", got, content)
	}

	fi, err := (Disk{}).Stat(p)
	if err != nil {
		t.Fatalf("Stat: %v", err)
	}
	if fi.Size() != int64(len(content)) {
		t.Fatalf("Size = %d, want %d", fi.Size(), len(content))
	}
	if _, _, ok := FileID(fi); !ok {
		t.Fatal("FileID: want ok=true for a real file on unix")
	}

	// Atomic write must leave no temp file behind in the directory.
	entries, _ := os.ReadDir(dir)
	for _, e := range entries {
		if filepath.Ext(e.Name()) == ".tmp" || e.Name()[0] == '.' {
			t.Fatalf("atomic write left residue: %s", e.Name())
		}
	}
}

func TestDisk_RenameAndMissing(t *testing.T) {
	dir := t.TempDir()
	old := filepath.Join(dir, "a.md")
	dst := filepath.Join(dir, "b.md")
	if err := (Disk{}).WriteFile(old, []byte("x"), 0o644); err != nil {
		t.Fatalf("WriteFile: %v", err)
	}
	if err := (Disk{}).Rename(old, dst); err != nil {
		t.Fatalf("Rename: %v", err)
	}
	if _, err := (Disk{}).Stat(old); !errors.Is(err, fs.ErrNotExist) {
		t.Fatalf("old path Stat: want ErrNotExist, got %v", err)
	}
	if _, err := (Disk{}).Stat(dst); err != nil {
		t.Fatalf("new path Stat: %v", err)
	}
}

func TestMem_RoundTripAndMissing(t *testing.T) {
	m := NewMem()
	if _, err := m.ReadFile("/x/missing.md"); !errors.Is(err, fs.ErrNotExist) {
		t.Fatalf("missing ReadFile: want ErrNotExist, got %v", err)
	}
	if _, err := m.Stat("/x/missing.md"); !errors.Is(err, fs.ErrNotExist) {
		t.Fatalf("missing Stat: want ErrNotExist, got %v", err)
	}

	p := "/docs/note.md"
	if err := m.WriteFile(p, []byte("hello"), 0o644); err != nil {
		t.Fatalf("WriteFile: %v", err)
	}
	got, err := m.ReadFile(p)
	if err != nil || string(got) != "hello" {
		t.Fatalf("ReadFile = %q, %v", got, err)
	}
	fi, err := m.Stat(p)
	if err != nil {
		t.Fatalf("Stat: %v", err)
	}
	if fi.Size() != 5 {
		t.Fatalf("Size = %d, want 5", fi.Size())
	}
	inode, _, ok := FileID(fi)
	if !ok || inode == 0 {
		t.Fatalf("FileID: ok=%v inode=%d, want ok=true nonzero", ok, inode)
	}
}

// TestMem_RenamePreservesInode: identity must survive a rename so history is not
// orphaned (§1.4.6).
func TestMem_RenamePreservesInode(t *testing.T) {
	m := NewMem()
	if err := m.WriteFile("/a.md", []byte("x"), 0o644); err != nil {
		t.Fatalf("WriteFile: %v", err)
	}
	fiA, _ := m.Stat("/a.md")
	inoA, _, _ := FileID(fiA)

	if err := m.Rename("/a.md", "/b.md"); err != nil {
		t.Fatalf("Rename: %v", err)
	}
	fiB, err := m.Stat("/b.md")
	if err != nil {
		t.Fatalf("Stat after rename: %v", err)
	}
	inoB, _, _ := FileID(fiB)
	if inoA != inoB {
		t.Fatalf("inode changed across rename: %d → %d (identity must be stable)", inoA, inoB)
	}
}

// TestMem_ReadDir lists direct files and synthesizes intermediate directories
// from the flat path map, sorted by name (like os.ReadDir).
func TestMem_ReadDir(t *testing.T) {
	m := NewMem()
	for _, p := range []string{"/a/y.md", "/a/x.md", "/a/sub/z.md"} {
		if err := m.WriteFile(p, []byte("x"), 0o644); err != nil {
			t.Fatalf("WriteFile %s: %v", p, err)
		}
	}
	entries, err := m.ReadDir("/a")
	if err != nil {
		t.Fatalf("ReadDir: %v", err)
	}
	type want struct {
		name string
		dir  bool
	}
	got := make([]want, len(entries))
	for i, e := range entries {
		got[i] = want{e.Name(), e.IsDir()}
	}
	expect := []want{{"sub", true}, {"x.md", false}, {"y.md", false}} // sorted by name
	if len(got) != len(expect) {
		t.Fatalf("ReadDir entries = %+v, want %+v", got, expect)
	}
	for i := range expect {
		if got[i] != expect[i] {
			t.Fatalf("entry %d = %+v, want %+v (full: %+v)", i, got[i], expect[i], got)
		}
	}

	// Empty/unknown directory lists nothing without erroring.
	if e, err := m.ReadDir("/nonexistent"); err != nil || len(e) != 0 {
		t.Fatalf("ReadDir empty: entries=%v err=%v, want [] nil", e, err)
	}
}

// TestMem_ModTimeAdvances: two same-size writes must yield different ModTimes so
// the §1.4.7 external-change guard can detect the change.
func TestMem_ModTimeAdvances(t *testing.T) {
	m := NewMem()
	p := "/a.md"
	_ = m.WriteFile(p, []byte("aa"), 0o644)
	fi1, _ := m.Stat(p)
	_ = m.WriteFile(p, []byte("bb"), 0o644) // same size, different content
	fi2, _ := m.Stat(p)
	if !fi2.ModTime().After(fi1.ModTime()) {
		t.Fatalf("ModTime did not advance: %v → %v", fi1.ModTime(), fi2.ModTime())
	}
}
