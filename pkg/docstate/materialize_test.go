package docstate

import (
	"testing"

	"rune/pkg/vfs"
)

// newMatTestStore returns an in-memory Store wired to a fresh vfs.Mem (mem
// options let a test opt into WithChurnInodeOnWrite).
func newMatTestStore(t *testing.T, opts ...vfs.MemOption) (*Store, *vfs.Mem) {
	t.Helper()
	s, err := OpenInMemory(nil)
	if err != nil {
		t.Fatalf("OpenInMemory: %v", err)
	}
	t.Cleanup(func() { s.Close() })
	mem := vfs.NewMem(opts...)
	s.UseFS(mem)
	return s, mem
}

// TestMaterialize_CASRefusal: the target is mutated AFTER expect was
// recorded (Load), so Materialize's unconditional pre-write hash (step 1-2)
// must refuse: Committed==false, a Fresh observation of the ACTUAL live
// bytes is recorded, and the target is left completely untouched.
func TestMaterialize_CASRefusal(t *testing.T) {
	s, mem := newMatTestStore(t)
	const path = "/note.md"
	if err := mem.WriteFile(path, []byte("original"), 0o644); err != nil {
		t.Fatalf("seed file: %v", err)
	}

	loaded, err := s.Load(path)
	if err != nil {
		t.Fatalf("Load: %v", err)
	}
	expect, hasSaved, err := s.SavedObs(loaded.DocID)
	if err != nil || !hasSaved {
		t.Fatalf("SavedObs after Load: hasSaved=%v err=%v", hasSaved, err)
	}

	// Mutate the target AFTER expect was recorded — an external writer.
	if err := mem.WriteFile(path, []byte("mutated externally"), 0o644); err != nil {
		t.Fatalf("mutate target: %v", err)
	}

	result, err := s.Materialize(loaded.DocID, path, "our new content", expect.ID, 0, false)
	if err != nil {
		t.Fatalf("Materialize: %v", err)
	}
	if result.Committed {
		t.Fatal("Materialize: want Committed=false (CAS refusal), got true")
	}
	if result.Fresh.BlobHash != hashBytes([]byte("mutated externally")) {
		t.Fatalf("Fresh.BlobHash = %q, want hash of %q", result.Fresh.BlobHash, "mutated externally")
	}

	got, err := mem.ReadFile(path)
	if err != nil || string(got) != "mutated externally" {
		t.Fatalf("target mutated by refused Materialize: got %q, err %v", got, err)
	}
}

// TestMaterialize_InWindowRace_CommitsRacedWithCapture is the F5 swap-race
// regression: a fault hook mutates the target between Materialize's
// pre-write hash and the atomic swap. The swap still occurs (Exchange
// doesn't know about the race) and OUR bytes physically land at the target —
// so Materialize must COMMIT (Committed=true, Raced=true) rather than
// discard our own already-landed write, while ALSO durably capturing the
// raced writer's displaced bytes (Fresh, capture-before-discard/I1) and
// removing the orphaned temp only after both records commit. Pre-fix,
// Materialize discarded its own successful write (Committed=false) and left
// the swapped-out temp file orphaned on disk forever.
func TestMaterialize_InWindowRace_CommitsRacedWithCapture(t *testing.T) {
	s, mem := newMatTestStore(t)
	const path = "/note.md"
	if err := mem.WriteFile(path, []byte("original"), 0o644); err != nil {
		t.Fatalf("seed file: %v", err)
	}

	loaded, err := s.Load(path)
	if err != nil {
		t.Fatalf("Load: %v", err)
	}
	expect, _, err := s.SavedObs(loaded.DocID)
	if err != nil {
		t.Fatalf("SavedObs: %v", err)
	}

	const ourContent = "our new content"
	raced := false
	hook := &hookFS{FS: mem, beforeExchange: func(real vfs.FS) {
		if raced {
			return
		}
		raced = true
		// A concurrent writer races us INSIDE the window between our
		// pre-write hash check (already passed) and the swap.
		if err := real.WriteFile(path, []byte("raced in"), 0o644); err != nil {
			t.Fatalf("race hook write: %v", err)
		}
	}}
	s.UseFS(hook)

	result, err := s.Materialize(loaded.DocID, path, ourContent, expect.ID, 0, false)
	if err != nil {
		t.Fatalf("Materialize: %v", err)
	}
	if !raced {
		t.Fatal("setup: race hook never fired")
	}

	// (nothing) our write must COMMIT — it's already physically on disk.
	if !result.Committed {
		t.Fatal("Materialize: want Committed=true (our bytes are already physically on disk after the swap)")
	}
	if !result.Raced {
		t.Fatal("Materialize: want Raced=true — a swap-race happened")
	}
	if result.Saved.BlobHash != hashBytes([]byte(ourContent)) {
		t.Fatalf("Saved.BlobHash = %q, want hash of OUR written content %q", result.Saved.BlobHash, ourContent)
	}
	if result.Fresh.BlobHash != hashBytes([]byte("raced in")) {
		t.Fatalf("Fresh.BlobHash = %q, want hash of the displaced %q", result.Fresh.BlobHash, "raced in")
	}

	// (a) The displaced ("raced in") bytes physically exist as a retrievable
	// blob, by the hash Materialize recorded — capture-before-discard (I1).
	displaced, err := s.GetBlob(result.Fresh.BlobHash)
	if err != nil {
		t.Fatalf("GetBlob(displaced): %v — capture-before-discard violated, bytes lost", err)
	}
	if displaced != "raced in" {
		t.Fatalf("displaced blob content = %q, want %q", displaced, "raced in")
	}

	// (c) saved_obs == the just-written bytes (the CAS record matches
	// physical reality — our bytes really are what's on disk now).
	savedObs, hasSaved, err := s.SavedObs(loaded.DocID)
	if err != nil || !hasSaved {
		t.Fatalf("SavedObs after race: hasSaved=%v err=%v", hasSaved, err)
	}
	if savedObs.ID != result.Saved.ID || savedObs.BlobHash != hashBytes([]byte(ourContent)) {
		t.Fatalf("saved_obs after race = %+v, want the just-committed observation for %q", savedObs, ourContent)
	}
	got, err := mem.ReadFile(path)
	if err != nil || string(got) != ourContent {
		t.Fatalf("disk content after race: got %q, err %v, want OUR content %q", got, err, ourContent)
	}

	// (b) the swapped-out temp is removed — no orphan left behind.
	if n := len(listMemPaths(mem)); n != 1 {
		t.Fatalf("mem holds %d paths after race, want exactly 1 (path) — an orphaned temp was left behind", n)
	}
}

// listMemPaths returns every path currently stored in mem, for asserting no
// orphaned temp files are left behind.
func listMemPaths(mem *vfs.Mem) []string {
	var paths []string
	entries, err := mem.ReadDir("/")
	if err != nil {
		return nil
	}
	for _, e := range entries {
		if !e.IsDir() {
			paths = append(paths, "/"+e.Name())
		}
	}
	return paths
}

// TestMaterialize_SuccessPath: a clean CAS write commits: the save
// observation's hash is EXACTLY the hash of the bytes passed to Materialize
// (never a re-read — closes G4), saved_obs advances to it, and the inode is
// rebound (mirrors Disk's atomic temp-then-rename churn, exercised here with
// WithChurnInodeOnWrite so even an in-place-looking write gets a fresh
// inode).
func TestMaterialize_SuccessPath(t *testing.T) {
	s, mem := newMatTestStore(t, vfs.WithChurnInodeOnWrite())
	const path = "/note.md"
	if err := mem.WriteFile(path, []byte("original"), 0o644); err != nil {
		t.Fatalf("seed file: %v", err)
	}

	loaded, err := s.Load(path)
	if err != nil {
		t.Fatalf("Load: %v", err)
	}
	expect, _, err := s.SavedObs(loaded.DocID)
	if err != nil {
		t.Fatalf("SavedObs: %v", err)
	}
	beforeInode := expect.Inode

	const newContent = "new content"
	result, err := s.Materialize(loaded.DocID, path, newContent, expect.ID, 0, false)
	if err != nil {
		t.Fatalf("Materialize: %v", err)
	}
	if !result.Committed {
		t.Fatal("Materialize: want Committed=true")
	}
	if result.Saved.BlobHash != hashBytes([]byte(newContent)) {
		t.Fatalf("Saved.BlobHash = %q, want hash of the WRITTEN content %q (not a re-read)", result.Saved.BlobHash, newContent)
	}

	savedObs, hasSaved, err := s.SavedObs(loaded.DocID)
	if err != nil || !hasSaved {
		t.Fatalf("SavedObs after Materialize: hasSaved=%v err=%v", hasSaved, err)
	}
	if savedObs.ID != result.Saved.ID {
		t.Fatalf("saved_obs = %d, want the just-committed observation %d", savedObs.ID, result.Saved.ID)
	}
	// D12/D13/§1.7: Inode is sql.NullInt64 — vfs.Mem's FileID always reports
	// ok=true (see vfs.FileID's doc comment), so a successful post-write stat
	// through Mem must yield Valid=true, never a NULL identity by accident.
	if !savedObs.Inode.Valid {
		t.Fatal("inode not recorded: savedObs.Inode.Valid = false after a successful Materialize write")
	}
	if savedObs.Inode == beforeInode {
		t.Fatalf("inode not rebound: still %+v after Materialize (want a fresh inode post-swap)", savedObs.Inode)
	}

	got, err := mem.ReadFile(path)
	if err != nil || string(got) != newContent {
		t.Fatalf("disk content after Materialize: got %q, err %v, want %q", got, err, newContent)
	}
}

// TestMaterialize_BindNew_Create: no target exists yet — Materialize creates
// it via RenameExcl (step 6), commits a save observation, and binds kind to
// 'file'.
func TestMaterialize_BindNew_Create(t *testing.T) {
	s, mem := newMatTestStore(t)
	ref, err := s.CreateScratch("Untitled")
	if err != nil {
		t.Fatalf("CreateScratch: %v", err)
	}
	const path = "/new.md"
	// documents.path is '' for a scratch doc — the real bind-new flow never
	// binds it before the first Materialize (workspace_update.go's
	// RenameRequestMsg handler); Materialize must create against the
	// caller-supplied path, never a DB-read one.
	var dbPath string
	if err := s.perm.QueryRow(`SELECT path FROM documents WHERE id=?`, ref.ID).Scan(&dbPath); err != nil {
		t.Fatalf("query path: %v", err)
	}
	if dbPath != "" {
		t.Fatalf("setup: documents.path = %q, want empty (scratch doc)", dbPath)
	}

	result, err := s.Materialize(ref.ID, path, "brand new content", 0, 0, true)
	if err != nil {
		t.Fatalf("Materialize: %v", err)
	}
	if !result.Committed {
		t.Fatal("Materialize (create): want Committed=true")
	}
	got, err := mem.ReadFile(path)
	if err != nil || string(got) != "brand new content" {
		t.Fatalf("disk content: got %q, err %v", got, err)
	}

	var kind string
	if err := s.perm.QueryRow(`SELECT kind FROM documents WHERE id=?`, ref.ID).Scan(&kind); err != nil {
		t.Fatalf("query kind: %v", err)
	}
	if kind != "file" {
		t.Errorf("kind after create: got %q, want %q", kind, "file")
	}
}

// TestMaterialize_BindNew_ConflictNoClobber: a concurrent creator wins the
// race — Materialize's RenameExcl refuses (no clobber, closes G1) and the
// concurrent creator's bytes are left untouched.
func TestMaterialize_BindNew_ConflictNoClobber(t *testing.T) {
	s, mem := newMatTestStore(t)
	ref, err := s.CreateScratch("Untitled")
	if err != nil {
		t.Fatalf("CreateScratch: %v", err)
	}
	const path = "/new.md"
	// documents.path is '' for a scratch doc — the real bind-new flow never
	// binds it before the first Materialize (workspace_update.go's
	// RenameRequestMsg handler); Materialize must create against the
	// caller-supplied path, never a DB-read one.
	var dbPath string
	if err := s.perm.QueryRow(`SELECT path FROM documents WHERE id=?`, ref.ID).Scan(&dbPath); err != nil {
		t.Fatalf("query path: %v", err)
	}
	if dbPath != "" {
		t.Fatalf("setup: documents.path = %q, want empty (scratch doc)", dbPath)
	}

	raced := false
	hook := &hookFS{FS: mem, beforeRenameExcl: func(real vfs.FS) {
		if raced {
			return
		}
		raced = true
		if err := real.WriteFile(path, []byte("concurrent creator"), 0o644); err != nil {
			t.Fatalf("race hook write: %v", err)
		}
	}}
	s.UseFS(hook)

	result, err := s.Materialize(ref.ID, path, "our content", 0, 0, true)
	if err != nil {
		t.Fatalf("Materialize: %v", err)
	}
	if result.Committed {
		t.Fatal("Materialize (create race): want Committed=false, got true (clobbered a concurrent creator)")
	}
	got, err := mem.ReadFile(path)
	if err != nil || string(got) != "concurrent creator" {
		t.Fatalf("concurrent creator's bytes clobbered: got %q, err %v", got, err)
	}
}

// TestMaterialize_ExchangeUnsupportedFallback: on a platform where Exchange
// is unsupported, Materialize falls back to probe+rename (step 7) and still
// commits correctly when nothing raced the fallback's re-check.
func TestMaterialize_ExchangeUnsupportedFallback(t *testing.T) {
	s, mem := newMatTestStore(t)
	const path = "/note.md"
	if err := mem.WriteFile(path, []byte("original"), 0o644); err != nil {
		t.Fatalf("seed file: %v", err)
	}
	loaded, err := s.Load(path)
	if err != nil {
		t.Fatalf("Load: %v", err)
	}
	expect, _, err := s.SavedObs(loaded.DocID)
	if err != nil {
		t.Fatalf("SavedObs: %v", err)
	}

	s.UseFS(unsupportedExchangeFS{FS: mem})

	result, err := s.Materialize(loaded.DocID, path, "via fallback", expect.ID, 0, false)
	if err != nil {
		t.Fatalf("Materialize: %v", err)
	}
	if !result.Committed {
		t.Fatal("Materialize (fallback): want Committed=true")
	}
	got, err := mem.ReadFile(path)
	if err != nil || string(got) != "via fallback" {
		t.Fatalf("disk content after fallback: got %q, err %v", got, err)
	}
}
