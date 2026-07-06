package vfs

import (
	"errors"
	"io/fs"
	"os"
	"path/filepath"
	"testing"
)

// fsFixture pairs an FS implementation with a path builder so table-driven
// tests exercise Disk (t.TempDir()) and Mem (synthetic keys) identically.
type fsFixture struct {
	name string
	fs   FS
	path func(leaf string) string
}

func exchangeFixtures(t *testing.T) []fsFixture {
	t.Helper()
	dir := t.TempDir()
	return []fsFixture{
		{name: "Disk", fs: Disk{}, path: func(leaf string) string { return filepath.Join(dir, leaf) }},
		{name: "Mem", fs: NewMem(), path: func(leaf string) string { return "/" + leaf }},
	}
}

// TestExchange_SwapsContentAndIdentity: after Exchange(a,b), the byte content
// at each path is swapped AND the stable identity (inode) travels with the
// content it used to hold — mirrors a physical renamex_np(RENAME_SWAP), and
// both byte-states remain present on the FS (§Part III I1: capture-before-
// discard). Table-driven over Disk and Mem so both implementations are held to
// the same observable contract.
func TestExchange_SwapsContentAndIdentity(t *testing.T) {
	for _, fx := range exchangeFixtures(t) {
		t.Run(fx.name, func(t *testing.T) {
			a, b := fx.path("a.md"), fx.path("b.md")
			if err := fx.fs.WriteFile(a, []byte("AAA"), 0o644); err != nil {
				t.Fatalf("WriteFile a: %v", err)
			}
			if err := fx.fs.WriteFile(b, []byte("BBB"), 0o644); err != nil {
				t.Fatalf("WriteFile b: %v", err)
			}
			fiA, err := fx.fs.Stat(a)
			if err != nil {
				t.Fatalf("Stat a: %v", err)
			}
			fiB, err := fx.fs.Stat(b)
			if err != nil {
				t.Fatalf("Stat b: %v", err)
			}
			inoA, _, okA := FileID(fiA)
			inoB, _, okB := FileID(fiB)
			if !okA || !okB {
				t.Fatalf("FileID unavailable: okA=%v okB=%v", okA, okB)
			}

			if err := fx.fs.Exchange(a, b); err != nil {
				t.Fatalf("Exchange: %v", err)
			}

			gotA, err := fx.fs.ReadFile(a)
			if err != nil {
				t.Fatalf("ReadFile a after exchange: %v", err)
			}
			gotB, err := fx.fs.ReadFile(b)
			if err != nil {
				t.Fatalf("ReadFile b after exchange: %v", err)
			}
			if string(gotA) != "BBB" || string(gotB) != "AAA" {
				t.Fatalf("contents not swapped: a=%q b=%q, want a=BBB b=AAA", gotA, gotB)
			}
			// Both byte-states are still present on the FS post-swap.
			if string(gotA) != "BBB" && string(gotB) != "BBB" {
				t.Fatal("BBB content vanished across the swap")
			}
			if string(gotA) != "AAA" && string(gotB) != "AAA" {
				t.Fatal("AAA content vanished across the swap")
			}

			fiA2, err := fx.fs.Stat(a)
			if err != nil {
				t.Fatalf("Stat a post-exchange: %v", err)
			}
			fiB2, err := fx.fs.Stat(b)
			if err != nil {
				t.Fatalf("Stat b post-exchange: %v", err)
			}
			inoA2, _, _ := FileID(fiA2)
			inoB2, _, _ := FileID(fiB2)
			if inoA2 != inoB || inoB2 != inoA {
				t.Fatalf("identity did not travel with content: a %d->%d (want %d), b %d->%d (want %d)",
					inoA, inoA2, inoB, inoB, inoB2, inoA)
			}
		})
	}
}

// TestExchange_MissingSide errors when either path does not exist; neither
// argument is created as a side effect.
func TestExchange_MissingSide(t *testing.T) {
	for _, fx := range exchangeFixtures(t) {
		t.Run(fx.name, func(t *testing.T) {
			a, b := fx.path("only-a.md"), fx.path("missing-b.md")
			if err := fx.fs.WriteFile(a, []byte("x"), 0o644); err != nil {
				t.Fatalf("WriteFile: %v", err)
			}
			if err := fx.fs.Exchange(a, b); err == nil {
				t.Fatal("Exchange with a missing side: want error, got nil")
			}
			if got, err := fx.fs.ReadFile(a); err != nil || string(got) != "x" {
				t.Fatalf("a mutated by failed Exchange: got %q, err %v", got, err)
			}
		})
	}
}

// TestRenameExcl_NoClobber: an existing target refuses the rename and its
// bytes are left completely untouched.
func TestRenameExcl_NoClobber(t *testing.T) {
	for _, fx := range exchangeFixtures(t) {
		t.Run(fx.name, func(t *testing.T) {
			oldPath, newPath := fx.path("old.md"), fx.path("existing.md")
			if err := fx.fs.WriteFile(oldPath, []byte("new bytes"), 0o644); err != nil {
				t.Fatalf("WriteFile old: %v", err)
			}
			if err := fx.fs.WriteFile(newPath, []byte("original bytes"), 0o644); err != nil {
				t.Fatalf("WriteFile new: %v", err)
			}

			err := fx.fs.RenameExcl(oldPath, newPath)
			if err == nil {
				t.Fatal("RenameExcl over existing target: want error, got nil")
			}
			if !errors.Is(err, fs.ErrExist) {
				t.Fatalf("RenameExcl error = %v, want wrapping fs.ErrExist", err)
			}

			got, rerr := fx.fs.ReadFile(newPath)
			if rerr != nil || string(got) != "original bytes" {
				t.Fatalf("target mutated by failed RenameExcl: got %q, err %v", got, rerr)
			}
			if _, rerr := fx.fs.ReadFile(oldPath); rerr != nil {
				t.Fatalf("old path should survive a refused RenameExcl: %v", rerr)
			}
		})
	}
}

// TestRenameExcl_Succeeds: renaming onto a free path moves the bytes and
// removes the old path.
func TestRenameExcl_Succeeds(t *testing.T) {
	for _, fx := range exchangeFixtures(t) {
		t.Run(fx.name, func(t *testing.T) {
			oldPath, newPath := fx.path("src.md"), fx.path("dst.md")
			if err := fx.fs.WriteFile(oldPath, []byte("payload"), 0o644); err != nil {
				t.Fatalf("WriteFile: %v", err)
			}
			if err := fx.fs.RenameExcl(oldPath, newPath); err != nil {
				t.Fatalf("RenameExcl: %v", err)
			}
			got, err := fx.fs.ReadFile(newPath)
			if err != nil || string(got) != "payload" {
				t.Fatalf("ReadFile new: got %q, err %v", got, err)
			}
			if _, err := fx.fs.ReadFile(oldPath); !errors.Is(err, fs.ErrNotExist) {
				t.Fatalf("old path should be gone: err=%v", err)
			}
		})
	}
}

// TestRemove: deletes exactly the named path; a second Remove errors
// ErrNotExist.
func TestRemove(t *testing.T) {
	for _, fx := range exchangeFixtures(t) {
		t.Run(fx.name, func(t *testing.T) {
			p := fx.path("temp.md")
			if err := fx.fs.WriteFile(p, []byte("x"), 0o644); err != nil {
				t.Fatalf("WriteFile: %v", err)
			}
			if err := fx.fs.Remove(p); err != nil {
				t.Fatalf("Remove: %v", err)
			}
			if _, err := fx.fs.ReadFile(p); !errors.Is(err, fs.ErrNotExist) {
				t.Fatalf("ReadFile after Remove: err=%v, want ErrNotExist", err)
			}
			if err := fx.fs.Remove(p); !errors.Is(err, fs.ErrNotExist) {
				t.Fatalf("second Remove: err=%v, want ErrNotExist", err)
			}
		})
	}
}

// TestDiskResolve: a symlink in a real tempdir resolves to its target; a
// not-yet-existing leaf resolves its existing parent and keeps the leaf name
// (needed by Materialize's create path, which resolves before the file
// exists).
func TestDiskResolve(t *testing.T) {
	dir := t.TempDir()
	target := filepath.Join(dir, "real.md")
	if err := os.WriteFile(target, []byte("x"), 0o644); err != nil {
		t.Fatalf("seed target: %v", err)
	}
	link := filepath.Join(dir, "link.md")
	if err := os.Symlink(target, link); err != nil {
		t.Fatalf("Symlink: %v", err)
	}

	got, err := (Disk{}).Resolve(link)
	if err != nil {
		t.Fatalf("Resolve(link): %v", err)
	}
	wantTarget, _ := filepath.EvalSymlinks(target)
	if got != wantTarget {
		t.Fatalf("Resolve(link) = %q, want %q", got, wantTarget)
	}

	// Non-existent leaf under a real (non-symlinked) directory: parent
	// resolves, leaf name is preserved verbatim.
	fresh := filepath.Join(dir, "brand-new.md")
	got, err = (Disk{}).Resolve(fresh)
	if err != nil {
		t.Fatalf("Resolve(fresh): %v", err)
	}
	wantDir, _ := filepath.EvalSymlinks(dir)
	if got != filepath.Join(wantDir, "brand-new.md") {
		t.Fatalf("Resolve(fresh) = %q, want %q", got, filepath.Join(wantDir, "brand-new.md"))
	}
}

// TestMemResolve: Mem.Resolve is the identity function, verbatim.
func TestMemResolve(t *testing.T) {
	m := NewMem()
	const p = "/a/b/c.md"
	got, err := m.Resolve(p)
	if err != nil || got != p {
		t.Fatalf("Mem.Resolve(%q) = %q, %v; want %q, nil", p, got, err, p)
	}
}

// TestFileNLink_Disk: an ordinary file has nlink==1; a hardlinked file has
// nlink==2 (WP4's Materialize protocol surfaces a footer warning above 1 —
// a save through a hardlinked path silently forks the document).
func TestFileNLink_Disk(t *testing.T) {
	dir := t.TempDir()
	p := filepath.Join(dir, "a.md")
	if err := (Disk{}).WriteFile(p, []byte("x"), 0o644); err != nil {
		t.Fatalf("WriteFile: %v", err)
	}
	fi, err := (Disk{}).Stat(p)
	if err != nil {
		t.Fatalf("Stat: %v", err)
	}
	n, ok := FileNLink(fi)
	if !ok || n != 1 {
		t.Fatalf("FileNLink(ordinary file) = %d, %v; want 1, true", n, ok)
	}

	link := filepath.Join(dir, "b.md")
	if err := os.Link(p, link); err != nil {
		t.Fatalf("os.Link: %v", err)
	}
	fi2, err := (Disk{}).Stat(p)
	if err != nil {
		t.Fatalf("Stat after hardlink: %v", err)
	}
	n2, ok := FileNLink(fi2)
	if !ok || n2 != 2 {
		t.Fatalf("FileNLink(hardlinked file) = %d, %v; want 2, true", n2, ok)
	}
}

// TestFileNLink_Mem: Mem has no hardlink concept — every file reports 1.
func TestFileNLink_Mem(t *testing.T) {
	m := NewMem()
	if err := m.WriteFile("/a.md", []byte("x"), 0o644); err != nil {
		t.Fatalf("WriteFile: %v", err)
	}
	fi, err := m.Stat("/a.md")
	if err != nil {
		t.Fatalf("Stat: %v", err)
	}
	n, ok := FileNLink(fi)
	if !ok || n != 1 {
		t.Fatalf("FileNLink(Mem) = %d, %v; want 1, true", n, ok)
	}
}
