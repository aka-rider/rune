package docstate

import (
	"database/sql"
	"os"
	"path/filepath"
	"testing"
)

// TestBind_PreservesHistory verifies that materializing an untitled doc to a
// real file keeps its id and undo history (§1.4.6): naming an Untitled must not
// orphan the edits made while it was untitled.
func TestBind_PreservesHistory(t *testing.T) {
	s := NewTestStore(t)
	docID := testDoc(t, s)
	for _, ch := range []string{"h", "i"} {
		if _, err := s.AppendEdit(docID, singleInsert(ch), noCursors, noCursors); err != nil {
			t.Fatalf("AppendEdit: %v", err)
		}
	}

	path := filepath.Join(t.TempDir(), "notes.md")
	if err := os.WriteFile(path, []byte("hi"), 0o644); err != nil {
		t.Fatalf("write file: %v", err)
	}
	if err := s.Bind(docID, path); err != nil {
		t.Fatalf("Bind: %v", err)
	}

	// Opening the now-bound file must resolve to the SAME doc id (history intact).
	ref, err := s.OpenPath(path)
	if err != nil {
		t.Fatalf("OpenPath: %v", err)
	}
	if ref.ID != docID {
		t.Fatalf("bind orphaned history: OpenPath id %d != scratch id %d", ref.ID, docID)
	}
	edits, err := s.AllEdits(docID)
	if err != nil {
		t.Fatalf("AllEdits: %v", err)
	}
	if len(edits) == 0 {
		t.Fatal("undo history lost after Bind")
	}

	// Bind also promotes kind to 'file' — a bound document IS a file.
	var kind string
	if err := s.perm.QueryRow(`SELECT kind FROM documents WHERE id=?`, docID).Scan(&kind); err != nil {
		t.Fatalf("query kind: %v", err)
	}
	if kind != "file" {
		t.Errorf("kind after Bind: got %q, want %q", kind, "file")
	}
}

// TestBind_StableAcrossInodeChange pins the save→reopen identity bug: an atomic
// write (temp→rename) gives the file a NEW inode, so without re-binding the next
// OpenPath would treat it as a new file and orphan the doc's undo history. After
// Store.Bind (which the workspace calls on every save) the same path must still
// resolve to the same docID.
func TestBind_StableAcrossInodeChange(t *testing.T) {
	s := NewTestStore(t)
	dir := t.TempDir()
	path := filepath.Join(dir, "note.md")
	if err := os.WriteFile(path, []byte("v1"), 0o644); err != nil {
		t.Fatal(err)
	}

	ref1, err := s.OpenPath(path)
	if err != nil {
		t.Fatalf("OpenPath: %v", err)
	}
	if _, err := s.AppendEdit(ref1.ID, singleInsert("x"), noCursors, noCursors); err != nil {
		t.Fatalf("AppendEdit: %v", err)
	}

	ino1, _, _ := s.statID(path)
	// Simulate an atomic save: os.CreateTemp + os.Rename gives the path a new inode.
	if err := os.Remove(path); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte("v2"), 0o644); err != nil {
		t.Fatal(err)
	}
	if ino2, _, _ := s.statID(path); ino2 == ino1 {
		t.Skip("filesystem reused the inode; cannot exercise the churn on this platform")
	}

	// The workspace re-binds after every save.
	if err := s.Bind(ref1.ID, path); err != nil {
		t.Fatalf("Bind: %v", err)
	}

	ref2, err := s.OpenPath(path)
	if err != nil {
		t.Fatalf("OpenPath after save: %v", err)
	}
	if ref2.ID != ref1.ID {
		t.Fatalf("save+reopen orphaned history: docID %d != %d", ref2.ID, ref1.ID)
	}
	if ref2.RenamedFrom != "" {
		t.Fatalf("unexpected rename warning after in-place save: %q", ref2.RenamedFrom)
	}
}

// TestBind_StatFailureClearsStaleIdentity is B5's regression test: Bind's
// stat-failed branch used to rebind path/kind while leaving the row's OLD
// inode/device untouched — a stale identity (§1.4.6). A doc that already
// holds a real inode (from an earlier successful Bind) must have it cleared
// to NULL, never left in place, when a later Bind's stat fails.
func TestBind_StatFailureClearsStaleIdentity(t *testing.T) {
	s := NewTestStore(t)
	dir := t.TempDir()
	path := filepath.Join(dir, "note.md")
	if err := os.WriteFile(path, []byte("v1"), 0o644); err != nil {
		t.Fatal(err)
	}

	docID := testDoc(t, s)
	if err := s.Bind(docID, path); err != nil {
		t.Fatalf("Bind (real file): %v", err)
	}
	if identityNull(t, s, docID) {
		t.Fatal("setup: doc should have a real (non-NULL) inode after binding to an existing file")
	}

	// Rebind to a path whose stat fails (parent directory doesn't exist).
	missingPath := filepath.Join(dir, "does-not-exist", "note.md")
	if err := s.Bind(docID, missingPath); err != nil {
		t.Fatalf("Bind (stat-failed path): %v", err)
	}
	if !identityNull(t, s, docID) {
		t.Fatal("Bind's stat-failed branch left the OLD inode/device in place instead of clearing to NULL (stale identity, §1.4.6)")
	}
	var gotPath string
	if err := s.perm.QueryRow(`SELECT path FROM documents WHERE id=?`, docID).Scan(&gotPath); err != nil {
		t.Fatalf("query path: %v", err)
	}
	if gotPath != missingPath {
		t.Fatalf("path not rebound: got %q, want %q", gotPath, missingPath)
	}
}

// TestRecoverableScratch_ExcludesZombies verifies that launch recovery returns
// only GENUINE untitled scratches (inode IS NULL), never an orphaned bound
// document whose path was cleared but which still carries a real inode —
// otherwise real-file content surfaces as a fake "Untitled" tab (the
// corruption regression).
func TestRecoverableScratch_ExcludesZombies(t *testing.T) {
	s := NewTestStore(t)

	// Genuine untitled scratch (inode IS NULL, §1.7) with content — recoverable.
	genuine := testDoc(t, s)
	if _, err := s.CreateSnapshot(genuine, "real note", 0); err != nil {
		t.Fatalf("CreateSnapshot genuine: %v", err)
	}

	// Zombie: an orphaned BOUND doc — path='' but a real (non-NULL) inode. Must be excluded.
	res, err := s.perm.Exec(
		`INSERT INTO documents(path, inode, device, created_at, last_seen_at) VALUES('', 999999, 1, '', '')`,
	)
	if err != nil {
		t.Fatalf("insert zombie: %v", err)
	}
	zombie, _ := res.LastInsertId()
	if _, err := s.CreateSnapshot(zombie, "secret real-file content", 0); err != nil {
		t.Fatalf("CreateSnapshot zombie: %v", err)
	}

	ids, err := s.RecoverableScratch(0)
	if err != nil {
		t.Fatalf("RecoverableScratch: %v", err)
	}
	has := func(id int64) bool {
		for _, x := range ids {
			if x == id {
				return true
			}
		}
		return false
	}
	if !has(genuine) {
		t.Errorf("genuine scratch missing from RecoverableScratch: %v", ids)
	}
	if has(zombie) {
		t.Errorf("zombie bound-doc (real inode) wrongly returned as recoverable: %v", ids)
	}
}

// TestGCEmptyScratch_KeepsHistoryAndLive verifies the launch GC removes only
// empty unbound docs — never one with history, never the live doc.
func TestGCEmptyScratch_KeepsHistoryAndLive(t *testing.T) {
	s := NewTestStore(t)
	live := testDoc(t, s)     // empty, but the live buffer — must survive
	empty := testDoc(t, s)    // empty, no history — should be collected
	withHist := testDoc(t, s) // has an event — must survive
	if _, err := s.AppendEdit(withHist, singleInsert("a"), noCursors, noCursors); err != nil {
		t.Fatalf("AppendEdit: %v", err)
	}

	if _, err := s.GCEmptyScratch(live); err != nil {
		t.Fatalf("GCEmptyScratch: %v", err)
	}

	if n := countRows(t, s.perm, `SELECT COUNT(*) FROM documents WHERE id=?`, live); n != 1 {
		t.Error("GC removed the live doc")
	}
	if n := countRows(t, s.perm, `SELECT COUNT(*) FROM documents WHERE id=?`, withHist); n != 1 {
		t.Error("GC removed a doc with history")
	}
	if n := countRows(t, s.perm, `SELECT COUNT(*) FROM documents WHERE id=?`, empty); n != 0 {
		t.Error("GC did not remove the empty scratch doc")
	}
}

// TestDeleteDocCascades pins the FK ON DELETE CASCADE that DeleteDoc relies on:
// deleting a document must remove its events and snapshots in one statement. If
// FK enforcement were silently off, those rows would orphan and resurface as
// phantom "recovered" scratch tabs (§1.4.6 identity corruption).
func TestDeleteDocCascades(t *testing.T) {
	s := NewTestStore(t)
	docID := testDoc(t, s)
	if _, err := s.AppendEdit(docID, singleInsert("a"), noCursors, noCursors); err != nil {
		t.Fatalf("AppendEdit: %v", err)
	}
	if _, err := s.CreateSnapshot(docID, "a", 0); err != nil {
		t.Fatalf("CreateSnapshot: %v", err)
	}
	if n := countRows(t, s.perm, `SELECT COUNT(*) FROM events WHERE doc_id=?`, docID); n == 0 {
		t.Fatal("precondition: expected events before delete")
	}
	if n := countRows(t, s.perm, `SELECT COUNT(*) FROM snapshots WHERE doc_id=?`, docID); n == 0 {
		t.Fatal("precondition: expected a snapshot before delete")
	}

	if err := s.DeleteDoc(docID); err != nil {
		t.Fatalf("DeleteDoc: %v", err)
	}

	if n := countRows(t, s.perm, `SELECT COUNT(*) FROM events WHERE doc_id=?`, docID); n != 0 {
		t.Errorf("CASCADE failed: %d orphaned events remain (FK enforcement off?)", n)
	}
	if n := countRows(t, s.perm, `SELECT COUNT(*) FROM snapshots WHERE doc_id=?`, docID); n != 0 {
		t.Errorf("CASCADE failed: %d orphaned snapshots remain (FK enforcement off?)", n)
	}
}

// TestOpenPath_SetsKindFile / TestCreateScratch_SetsKindScratch /
// TestReserveChatDoc_SetsKindChat pin the per-constructor kind tagging WP3
// adds (documents.kind: 'file' | 'scratch' | 'chat').
func TestOpenPath_SetsKindFile(t *testing.T) {
	s := NewTestStore(t)
	path := filepath.Join(t.TempDir(), "f.md")
	if err := os.WriteFile(path, []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}
	ref, err := s.OpenPath(path)
	if err != nil {
		t.Fatalf("OpenPath: %v", err)
	}
	var kind string
	if err := s.perm.QueryRow(`SELECT kind FROM documents WHERE id=?`, ref.ID).Scan(&kind); err != nil {
		t.Fatalf("query kind: %v", err)
	}
	if kind != "file" {
		t.Errorf("kind: got %q, want %q", kind, "file")
	}
}

func TestCreateScratch_SetsKindScratch(t *testing.T) {
	s := NewTestStore(t)
	ref, err := s.CreateScratch("Untitled")
	if err != nil {
		t.Fatalf("CreateScratch: %v", err)
	}
	var kind string
	if err := s.perm.QueryRow(`SELECT kind FROM documents WHERE id=?`, ref.ID).Scan(&kind); err != nil {
		t.Fatalf("query kind: %v", err)
	}
	if kind != "scratch" {
		t.Errorf("kind: got %q, want %q", kind, "scratch")
	}
}

func TestReserveChatDoc_SetsKindChat(t *testing.T) {
	s := NewTestStore(t)
	chatID, err := s.ReserveChatDoc()
	if err != nil {
		t.Fatalf("ReserveChatDoc: %v", err)
	}
	var kind string
	if err := s.perm.QueryRow(`SELECT kind FROM documents WHERE id=?`, chatID).Scan(&kind); err != nil {
		t.Fatalf("query kind: %v", err)
	}
	if kind != "chat" {
		t.Errorf("kind: got %q, want %q", kind, "chat")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// §1.7 identity NULL — documents.inode/device record "identity unknown" as
// SQL NULL, never the literal 0 sentinel every write site used to write.
// ─────────────────────────────────────────────────────────────────────────────

// identityNull reports whether docID's inode column is NULL (the only
// "identity unknown" spelling post-fix — never a literal 0).
func identityNull(t *testing.T, s *Store, docID int64) bool {
	t.Helper()
	var inode sql.NullInt64
	if err := s.perm.QueryRow(`SELECT inode FROM documents WHERE id=?`, docID).Scan(&inode); err != nil {
		t.Fatalf("identityNull: query doc %d: %v", docID, err)
	}
	return !inode.Valid
}

// TestIdentityNull_CreateScratchWritesNull pins CreateScratch's write site:
// inode/device are NULL by omission, never a literal 0.
func TestIdentityNull_CreateScratchWritesNull(t *testing.T) {
	s := NewTestStore(t)
	ref, err := s.CreateScratch("Untitled")
	if err != nil {
		t.Fatalf("CreateScratch: %v", err)
	}
	if !identityNull(t, s, ref.ID) {
		t.Error("CreateScratch: inode is not NULL (still writing the literal-0 sentinel?)")
	}
}

// TestIdentityNull_ReserveChatDocWritesNull pins ReserveChatDoc's write site.
func TestIdentityNull_ReserveChatDocWritesNull(t *testing.T) {
	s := NewTestStore(t)
	chatID, err := s.ReserveChatDoc()
	if err != nil {
		t.Fatalf("ReserveChatDoc: %v", err)
	}
	if !identityNull(t, s, chatID) {
		t.Error("ReserveChatDoc: inode is not NULL (still writing the literal-0 sentinel?)")
	}
}

// TestIdentityNull_TwoScratchDocsCoexist is the regression test named in the
// plan: two identity-less documents (both created via CreateScratch, neither
// ever bound to a real file) must NOT collide on idx_documents_inode. A
// partial-conversion bug (only some write sites moved to NULL while the
// unique index still excluded literal 0 as a special case) risks exactly
// this: two identity-less rows both landing in the index once it drops its
// "AND inode != 0" carve-out, producing a UNIQUE constraint violation on the
// second insert.
func TestIdentityNull_TwoScratchDocsCoexist(t *testing.T) {
	s := NewTestStore(t)
	ref1, err := s.CreateScratch("Untitled 1")
	if err != nil {
		t.Fatalf("CreateScratch 1: %v", err)
	}
	ref2, err := s.CreateScratch("Untitled 2")
	if err != nil {
		t.Fatalf("CreateScratch 2 (must not UNIQUE-collide with doc 1 on inode/device): %v", err)
	}
	if ref1.ID == ref2.ID {
		t.Fatalf("two CreateScratch calls returned the same docID: %d", ref1.ID)
	}
	if !identityNull(t, s, ref1.ID) || !identityNull(t, s, ref2.ID) {
		t.Error("both scratch docs must have NULL inode")
	}
	if n := countRows(t, s.perm, `SELECT COUNT(*) FROM documents WHERE inode IS NULL`); n < 2 {
		t.Errorf("expected at least 2 NULL-identity documents rows, got %d", n)
	}
}

// TestIdentityNull_PathKeyedDocFoundOnReopen is the "path-keyed docs found"
// regression: a document created via the path-fallback (openPathByName, no
// real inode available — e.g. a nonexistent path) must resolve to the SAME
// docID on a second OpenPath, via the `inode IS NULL` lookup query (not the
// old `inode IS NULL OR inode=0`), never silently creating a duplicate row.
func TestIdentityNull_PathKeyedDocFoundOnReopen(t *testing.T) {
	s := NewTestStore(t)
	const path = "/nonexistent/never-on-disk.md"

	ref1, err := s.OpenPath(path)
	if err != nil {
		t.Fatalf("OpenPath (1st, path-fallback): %v", err)
	}
	if !identityNull(t, s, ref1.ID) {
		t.Fatal("path-keyed doc must have NULL inode")
	}

	ref2, err := s.OpenPath(path)
	if err != nil {
		t.Fatalf("OpenPath (2nd): %v", err)
	}
	if ref2.ID != ref1.ID {
		t.Fatalf("path-keyed doc not found on reopen: got docID %d, want %d (a duplicate row was created)", ref2.ID, ref1.ID)
	}
	if n := countRows(t, s.perm, `SELECT COUNT(*) FROM documents WHERE path=?`, path); n != 1 {
		t.Fatalf("expected exactly 1 documents row for the path-keyed path, got %d", n)
	}
}

// TestIdentityNull_EvictedDocFoundAsRecoverableScratch is the "evicted docs
// found" regression: Bind's stale-inode-holder eviction clears a row's
// inode/device to NULL (never leaves them at a literal 0), and that evicted
// row — now identity-less and path-less, exactly like a genuine scratch doc —
// must still be found by RecoverableScratch's `inode IS NULL` filter (not
// silently lost, and not misclassified as a "zombie" that gets excluded).
func TestIdentityNull_EvictedDocFoundAsRecoverableScratch(t *testing.T) {
	s := NewTestStore(t)
	path := filepath.Join(t.TempDir(), "shared.md")
	if err := os.WriteFile(path, []byte("v1"), 0o644); err != nil {
		t.Fatal(err)
	}

	// doc1 binds to path first, adopting its real inode.
	doc1 := testDoc(t, s)
	if err := s.Bind(doc1, path); err != nil {
		t.Fatalf("Bind doc1: %v", err)
	}
	if _, err := s.AppendEdit(doc1, singleInsert("unsaved work"), noCursors, noCursors); err != nil {
		t.Fatalf("AppendEdit doc1: %v", err)
	}
	if identityNull(t, s, doc1) {
		t.Fatal("setup: doc1 should have a real (non-NULL) inode after Bind")
	}

	// doc2 binds to the SAME path — Bind's eviction step must clear doc1's
	// now-stale inode/device claim to NULL rather than leaving/writing 0.
	doc2 := testDoc(t, s)
	if err := s.Bind(doc2, path); err != nil {
		t.Fatalf("Bind doc2: %v", err)
	}

	if !identityNull(t, s, doc1) {
		t.Fatal("doc1's inode must be evicted to NULL (not left/written as 0) once doc2 claims the same inode")
	}
	var doc1Path string
	if err := s.perm.QueryRow(`SELECT path FROM documents WHERE id=?`, doc1).Scan(&doc1Path); err != nil {
		t.Fatalf("query doc1 path: %v", err)
	}
	if doc1Path != "" {
		t.Fatalf("doc1's path should have been freed by doc2's Bind, got %q", doc1Path)
	}

	// The evicted doc1 — identity-less, path-less, but carrying real history —
	// must surface as a recoverable scratch, not be silently lost.
	ids, err := s.RecoverableScratch(0)
	if err != nil {
		t.Fatalf("RecoverableScratch: %v", err)
	}
	found := false
	for _, id := range ids {
		if id == doc1 {
			found = true
		}
	}
	if !found {
		t.Fatalf("evicted doc1 (docID %d) not found by RecoverableScratch: %v", doc1, ids)
	}
}
