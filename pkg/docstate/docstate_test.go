package docstate

import (
	"database/sql"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"rune/pkg/editor/buffer"
	"rune/pkg/editor/cursor"
)

// ---- helpers ----------------------------------------------------------------

// newStoreAtPath creates a Store backed by a real file at permPath.
// The caller is responsible for closing the store.
func newStoreAtPath(t *testing.T, permPath string) *Store {
	t.Helper()
	perm, err := openPerm(permPath)
	if err != nil {
		t.Fatalf("newStoreAtPath: openPerm %q: %v", permPath, err)
	}
	return &Store{perm: perm, clock: time.Now}
}

// countRows returns the row count for the given query.
func countRows(t *testing.T, db *sql.DB, query string, args ...any) int {
	t.Helper()
	var n int
	if err := db.QueryRow(query, args...).Scan(&n); err != nil {
		t.Fatalf("countRows %q: %v", query, err)
	}
	return n
}

// singleInsert returns a one-element AppliedEdit slice that looks like a
// single character insertion with no deleted text.
func singleInsert(ch string) []buffer.AppliedEdit {
	return []buffer.AppliedEdit{{Start: 0, End: 0, Deleted: "", Insert: ch}}
}

// testDoc creates a scratch document and returns its docID. Fatal on error.
func testDoc(t *testing.T, s *Store) int64 {
	t.Helper()
	ref, err := s.CreateScratch("")
	if err != nil {
		t.Fatalf("testDoc: CreateScratch: %v", err)
	}
	return ref.ID
}

var noCursors []cursor.Cursor

// ---- P0: integrity & persistence --------------------------------------------

func TestBlobRoundTrip(t *testing.T) {
	s := NewTestStore(t)

	cases := []struct {
		name    string
		content string
	}{
		{"ascii", "Hello, World! This is a test."},
		{"cjk", "日本語テスト"},
		{"emoji", "🎉🚀✨🌍"},
		{"large", strings.Repeat("abcdefghijklmnopqrstuvwxyz\n", 40000)}, // >1 MB
	}

	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			hash, err := s.PutBlob(tc.content)
			if err != nil {
				t.Fatalf("PutBlob: %v", err)
			}
			got, err := s.GetBlob(hash)
			if err != nil {
				t.Fatalf("GetBlob: %v", err)
			}
			if got != tc.content {
				t.Errorf("round-trip mismatch: got %d bytes, want %d bytes", len(got), len(tc.content))
			}
		})
	}
}

func TestBlobDeduplication(t *testing.T) {
	s := NewTestStore(t)
	content := "identical content"

	h1, err := s.PutBlob(content)
	if err != nil {
		t.Fatalf("first PutBlob: %v", err)
	}
	h2, err := s.PutBlob(content)
	if err != nil {
		t.Fatalf("second PutBlob: %v", err)
	}
	if h1 != h2 {
		t.Errorf("expected same hash for identical content, got %q vs %q", h1, h2)
	}

	n := countRows(t, s.perm, `SELECT COUNT(*) FROM blobs WHERE hash=?`, h1)
	if n != 1 {
		t.Errorf("expected exactly 1 blobs row, got %d", n)
	}
}

func TestFlushWritesSnapshot(t *testing.T) {
	s := NewTestStore(t)
	content := "the quick brown fox"

	ref, err := s.OpenPath("/test.md")
	if err != nil {
		t.Fatalf("OpenPath: %v", err)
	}
	docID := ref.ID

	snapID, err := s.CreateSnapshot(docID, content, "local", 0)
	if err != nil {
		t.Fatalf("CreateSnapshot: %v", err)
	}
	if snapID <= 0 {
		t.Errorf("expected positive snapshot id, got %d", snapID)
	}

	// Assert exactly one snapshots row with correct doc_id, source.
	var gotDocID int64
	var gotSource string
	if err := s.perm.QueryRow(
		`SELECT doc_id, source FROM snapshots WHERE id=?`, snapID,
	).Scan(&gotDocID, &gotSource); err != nil {
		t.Fatalf("query snapshots: %v", err)
	}
	if gotDocID != docID {
		t.Errorf("doc_id: got %d, want %d", gotDocID, docID)
	}
	if gotSource != "local" {
		t.Errorf("source: got %q, want %q", gotSource, "local")
	}

	// LatestSnapshot reconstructs the original content.
	got, err := s.LatestSnapshot(docID)
	if err != nil {
		t.Fatalf("LatestSnapshot: %v", err)
	}
	if got != content {
		t.Errorf("LatestSnapshot: got %q, want %q", got, content)
	}
}

func TestCrashRecovery(t *testing.T) {
	permPath := filepath.Join(t.TempDir(), "crash_test.db")

	const content = "document content after crash"
	const draftText = "in-progress chat prompt"
	var savedDocID int64

	// Session 1: write data and close.
	{
		s := newStoreAtPath(t, permPath)
		ref, err := s.OpenPath("/notes.md")
		if err != nil {
			t.Fatalf("OpenPath: %v", err)
		}
		savedDocID = ref.ID

		edits := singleInsert("a")
		if _, err := s.AppendEdit(savedDocID, "main", edits, noCursors, noCursors, "main"); err != nil {
			t.Fatalf("AppendEdit: %v", err)
		}
		if _, err := s.CreateSnapshot(savedDocID, content, "local", 0); err != nil {
			t.Fatalf("CreateSnapshot: %v", err)
		}
		if err := s.UpsertDraft("chat", draftText); err != nil {
			t.Fatalf("UpsertDraft: %v", err)
		}
		if err := s.Close(); err != nil {
			t.Fatalf("Close: %v", err)
		}
	}

	// Session 2: reopen and verify persisted state.
	{
		s := newStoreAtPath(t, permPath)
		defer s.Close()

		got, err := s.LatestSnapshot(savedDocID)
		if err != nil {
			t.Fatalf("LatestSnapshot after reopen: %v", err)
		}
		if got != content {
			t.Errorf("crash recovery: got %q, want %q", got, content)
		}

		draft, err := s.GetDraft("chat")
		if err != nil {
			t.Fatalf("GetDraft after reopen: %v", err)
		}
		if draft != draftText {
			t.Errorf("draft recovery: got %q, want %q", draft, draftText)
		}
	}
}

func TestOpenLadderDegradation(t *testing.T) {
	// Create a dir with no permissions so MkdirAll fails.
	noPermDir := t.TempDir()
	if err := os.Chmod(noPermDir, 0o000); err != nil {
		t.Skip("cannot chmod temp dir:", err)
	}
	t.Cleanup(func() { os.Chmod(noPermDir, 0o700) }) // restore for cleanup

	t.Setenv("HOME", noPermDir)

	store, warn, err := Open()
	if err != nil {
		t.Fatalf("Open() returned error: %v", err)
	}
	defer store.Close()

	if !strings.Contains(warn, "history disabled") {
		t.Errorf("expected degradation warning containing %q, got %q", "history disabled", warn)
	}
	if store == nil {
		t.Fatal("Open() returned nil store despite warning")
	}

	// Store must be functional despite degradation.
	ref, err := store.OpenPath("/degraded.md")
	if err != nil {
		t.Fatalf("store not functional after degradation: OpenPath: %v", err)
	}
	if _, err := store.CreateSnapshot(ref.ID, "content", "local", 0); err != nil {
		t.Fatalf("store not functional after degradation: CreateSnapshot: %v", err)
	}
}

// ---- P1: undo/redo correctness (journal) ------------------------------------

func TestBasicUndo(t *testing.T) {
	s := NewTestStore(t)
	docID := testDoc(t, s)

	edits := singleInsert("a")
	before := []cursor.Cursor{{Position: 0, Anchor: 0}}
	after := []cursor.Cursor{{Position: 1, Anchor: 1}}

	if _, err := s.AppendEdit(docID, "main", edits, before, after, "main"); err != nil {
		t.Fatalf("AppendEdit: %v", err)
	}

	surface, gotEdits, gotBefore, _, ok := s.UndoTarget(docID)
	if !ok {
		t.Fatal("UndoTarget: expected ok=true, got false")
	}
	if surface != "main" {
		t.Errorf("surface: got %q, want %q", surface, "main")
	}
	if len(gotEdits) != 1 || gotEdits[0].Insert != "a" {
		t.Errorf("edits: got %+v, want insert=a", gotEdits)
	}
	if len(gotBefore) != 1 || gotBefore[0].Position != 0 {
		t.Errorf("cursorsBefore: got %+v, want position=0", gotBefore)
	}

	// Second undo: nothing left.
	_, _, _, _, ok2 := s.UndoTarget(docID)
	if ok2 {
		t.Error("second UndoTarget: expected ok=false (nothing left to undo)")
	}
}

func TestBasicRedo(t *testing.T) {
	s := NewTestStore(t)
	docID := testDoc(t, s)

	edits := singleInsert("b")
	after := []cursor.Cursor{{Position: 1, Anchor: 1}}

	if _, err := s.AppendEdit(docID, "main", edits, noCursors, after, "main"); err != nil {
		t.Fatalf("AppendEdit: %v", err)
	}

	// Undo.
	if _, _, _, _, ok := s.UndoTarget(docID); !ok {
		t.Fatal("UndoTarget returned ok=false")
	}

	// Redo.
	surface, gotEdits, gotAfter, _, ok := s.RedoTarget(docID)
	if !ok {
		t.Fatal("RedoTarget: expected ok=true, got false")
	}
	if surface != "main" {
		t.Errorf("surface: got %q, want %q", surface, "main")
	}
	if len(gotEdits) != 1 || gotEdits[0].Insert != "b" {
		t.Errorf("edits: got %+v, want insert=b", gotEdits)
	}
	if len(gotAfter) != 1 || gotAfter[0].Position != 1 {
		t.Errorf("cursorsAfter: got %+v, want position=1", gotAfter)
	}

	// Second redo: nothing left.
	_, _, _, _, ok2 := s.RedoTarget(docID)
	if ok2 {
		t.Error("second RedoTarget: expected ok=false (nothing left to redo)")
	}
}

// TestUndoUndoRedo verifies that the journal correctly returns a redo target
// after two consecutive undos. This is the journal-layer counterpart to the
// Reapply regression test in the textedit package.
func TestUndoUndoRedo(t *testing.T) {
	s := NewTestStore(t)
	docID := testDoc(t, s)

	now := time.Now()
	s.clock = func() time.Time { return now }

	// Event A.
	if _, err := s.AppendEdit(docID, "main", singleInsert("a"), noCursors, noCursors, "main"); err != nil {
		t.Fatalf("AppendEdit A: %v", err)
	}
	// Event B (outside 300ms window → separate undo-stop).
	now = now.Add(400 * time.Millisecond)
	if _, err := s.AppendEdit(docID, "main", singleInsert("b"), noCursors, noCursors, "main"); err != nil {
		t.Fatalf("AppendEdit B: %v", err)
	}

	// Undo B.
	if _, _, _, _, ok := s.UndoTarget(docID); !ok {
		t.Fatal("first UndoTarget returned ok=false")
	}
	// Undo A.
	_, gotEditsA, _, _, ok := s.UndoTarget(docID)
	if !ok {
		t.Fatal("second UndoTarget returned ok=false")
	}
	if len(gotEditsA) == 0 || gotEditsA[0].Insert != "a" {
		t.Errorf("second undo edits: got %+v, want insert=a", gotEditsA)
	}

	// Redo A — journal must return ok=true.
	_, gotRedoEdits, _, _, okRedo := s.RedoTarget(docID)
	if !okRedo {
		t.Fatal("RedoTarget after Undo×2: expected ok=true, got false")
	}
	if len(gotRedoEdits) == 0 || gotRedoEdits[0].Insert != "a" {
		t.Errorf("redo edits: got %+v, want insert=a", gotRedoEdits)
	}
}

func TestCoalescingWithinWindow(t *testing.T) {
	s := NewTestStore(t)
	docID := testDoc(t, s)

	// Fix the clock at t=0.
	now := time.Now()
	s.clock = func() time.Time { return now }

	if _, err := s.AppendEdit(docID, "main", singleInsert("a"), noCursors, noCursors, "main"); err != nil {
		t.Fatalf("AppendEdit 1: %v", err)
	}
	// Advance clock by 100ms (within 300ms window).
	now = now.Add(100 * time.Millisecond)
	if _, err := s.AppendEdit(docID, "main", singleInsert("b"), noCursors, noCursors, "main"); err != nil {
		t.Fatalf("AppendEdit 2: %v", err)
	}

	n := countRows(t, s.perm, `SELECT COUNT(*) FROM events WHERE doc_id=? AND is_undo_stop=1`, docID)
	if n != 1 {
		t.Errorf("expected 1 coalesced event row, got %d", n)
	}
}

func TestCoalescingOutsideWindow(t *testing.T) {
	s := NewTestStore(t)
	docID := testDoc(t, s)

	now := time.Now()
	s.clock = func() time.Time { return now }

	if _, err := s.AppendEdit(docID, "main", singleInsert("a"), noCursors, noCursors, "main"); err != nil {
		t.Fatalf("AppendEdit 1: %v", err)
	}
	// Advance clock by 400ms (outside 300ms window).
	now = now.Add(400 * time.Millisecond)
	if _, err := s.AppendEdit(docID, "main", singleInsert("b"), noCursors, noCursors, "main"); err != nil {
		t.Fatalf("AppendEdit 2: %v", err)
	}

	n := countRows(t, s.perm, `SELECT COUNT(*) FROM events WHERE doc_id=? AND is_undo_stop=1`, docID)
	if n != 2 {
		t.Errorf("expected 2 event rows (no coalesce), got %d", n)
	}
}

func TestCoalescingWhitespaceBreaks(t *testing.T) {
	s := NewTestStore(t)
	docID := testDoc(t, s)

	now := time.Now()
	s.clock = func() time.Time { return now }

	// First insert: non-whitespace.
	if _, err := s.AppendEdit(docID, "main", singleInsert("a"), noCursors, noCursors, "main"); err != nil {
		t.Fatalf("AppendEdit 1: %v", err)
	}
	// Second insert: space — must break coalesce.
	now = now.Add(50 * time.Millisecond)
	if _, err := s.AppendEdit(docID, "main", singleInsert(" "), noCursors, noCursors, "main"); err != nil {
		t.Fatalf("AppendEdit space: %v", err)
	}
	// Third insert after whitespace — new stop.
	now = now.Add(50 * time.Millisecond)
	if _, err := s.AppendEdit(docID, "main", singleInsert("b"), noCursors, noCursors, "main"); err != nil {
		t.Fatalf("AppendEdit 3: %v", err)
	}

	n := countRows(t, s.perm, `SELECT COUNT(*) FROM events WHERE doc_id=? AND is_undo_stop=1`, docID)
	// Sequence: [a] + [space] (space coalesces into 'a' event? No — space is inserted
	// AFTER 'a', so 'a' event's last insert is 'a' (non-whitespace), space coalesces into it.
	// Then 'b' tries to coalesce: last event's last insert is ' ' (whitespace) → no coalesce.
	// Result: 2 events: [a+space] and [b].
	if n != 2 {
		t.Errorf("expected 2 events (whitespace breaks next coalesce), got %d", n)
	}
}

func TestCoalescingSurfaceBreak(t *testing.T) {
	s := NewTestStore(t)
	docID := testDoc(t, s)

	now := time.Now()
	s.clock = func() time.Time { return now }

	if _, err := s.AppendEdit(docID, "main", singleInsert("a"), noCursors, noCursors, "main"); err != nil {
		t.Fatalf("AppendEdit main: %v", err)
	}
	// Different surface — must not coalesce.
	now = now.Add(50 * time.Millisecond)
	if _, err := s.AppendEdit(docID, "title", singleInsert("b"), noCursors, noCursors, "title"); err != nil {
		t.Fatalf("AppendEdit title: %v", err)
	}

	n := countRows(t, s.perm, `SELECT COUNT(*) FROM events WHERE doc_id=? AND is_undo_stop=1`, docID)
	if n != 2 {
		t.Errorf("expected 2 events (surface break prevents coalesce), got %d", n)
	}
}

func TestTruncateOnNewEdit(t *testing.T) {
	s3 := NewTestStore(t)
	docID := testDoc(t, s3)

	nowT := time.Now()
	s3.clock = func() time.Time { return nowT }

	for _, ch := range []string{"x", "y", "z"} {
		if _, err := s3.AppendEdit(docID, "main", singleInsert(ch), noCursors, noCursors, "main"); err != nil {
			t.Fatalf("AppendEdit %q: %v", ch, err)
		}
		nowT = nowT.Add(400 * time.Millisecond) // ensure separate stops
	}

	before := countRows(t, s3.perm, `SELECT COUNT(*) FROM events WHERE doc_id=?`, docID)
	if before != 3 {
		t.Fatalf("expected 3 events before undo, got %d", before)
	}

	// Undo once to step back.
	if _, _, _, _, ok := s3.UndoTarget(docID); !ok {
		t.Fatal("UndoTarget returned ok=false")
	}
	// Undo again.
	if _, _, _, _, ok := s3.UndoTarget(docID); !ok {
		t.Fatal("second UndoTarget returned ok=false")
	}

	// New edit after undo — must truncate the two future events.
	nowT = nowT.Add(400 * time.Millisecond)
	if _, err := s3.AppendEdit(docID, "main", singleInsert("w"), noCursors, noCursors, "main"); err != nil {
		t.Fatalf("AppendEdit after undo: %v", err)
	}

	after := countRows(t, s3.perm, `SELECT COUNT(*) FROM events WHERE doc_id=?`, docID)
	// Before undo we were at seq 3. After two undos current_seq = 1 (at or before seq 1).
	// New edit truncates seq > current_seq (seq 2 and 3), then inserts new event.
	// Remaining: seq 1 + new event = 2 events.
	if after != 2 {
		t.Errorf("expected 2 events after truncate-on-new-edit, got %d", after)
	}

	// Redo should now return false (future was truncated).
	_, _, _, _, ok := s3.RedoTarget(docID)
	if ok {
		t.Error("RedoTarget should return ok=false after truncate-on-new-edit")
	}
}

// TestBind_PreservesHistory verifies that materializing an untitled doc to a
// real file keeps its id and undo history (§1.4.6): naming an Untitled must not
// orphan the edits made while it was untitled.
func TestBind_PreservesHistory(t *testing.T) {
	s := NewTestStore(t)
	docID := testDoc(t, s)
	for _, ch := range []string{"h", "i"} {
		if _, err := s.AppendEdit(docID, "main", singleInsert(ch), noCursors, noCursors, "main"); err != nil {
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
	edits, err := s.AllEdits(docID, "main")
	if err != nil {
		t.Fatalf("AllEdits: %v", err)
	}
	if len(edits) == 0 {
		t.Fatal("undo history lost after Bind")
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
	if _, err := s.AppendEdit(ref1.ID, "main", singleInsert("x"), noCursors, noCursors, "main"); err != nil {
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

// TestRecoverableScratch_ExcludesZombies verifies that launch recovery returns
// only GENUINE untitled scratches (inode=0), never an orphaned bound document
// whose path was cleared (inode!=0) — otherwise real-file content surfaces as a
// fake "Untitled" tab (the corruption regression).
func TestRecoverableScratch_ExcludesZombies(t *testing.T) {
	s := NewTestStore(t)

	// Genuine untitled scratch (inode=0) with content — recoverable.
	genuine := testDoc(t, s)
	if _, err := s.CreateSnapshot(genuine, "real note", "local", 0); err != nil {
		t.Fatalf("CreateSnapshot genuine: %v", err)
	}

	// Zombie: an orphaned BOUND doc — path='' but a real inode. Must be excluded.
	res, err := s.perm.Exec(
		`INSERT INTO documents(path, inode, device, created_at, last_seen_at) VALUES('', 999999, 1, '', '')`,
	)
	if err != nil {
		t.Fatalf("insert zombie: %v", err)
	}
	zombie, _ := res.LastInsertId()
	if _, err := s.CreateSnapshot(zombie, "secret real-file content", "local", 0); err != nil {
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
		t.Errorf("zombie bound-doc (inode!=0) wrongly returned as recoverable: %v", ids)
	}
}

// TestGCEmptyScratch_KeepsHistoryAndLive verifies the launch GC removes only
// empty unbound docs — never one with history, never the live doc.
func TestGCEmptyScratch_KeepsHistoryAndLive(t *testing.T) {
	s := NewTestStore(t)
	live := testDoc(t, s)    // empty, but the live buffer — must survive
	empty := testDoc(t, s)   // empty, no history — should be collected
	withHist := testDoc(t, s) // has an event — must survive
	if _, err := s.AppendEdit(withHist, "main", singleInsert("a"), noCursors, noCursors, "main"); err != nil {
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
	if _, err := s.AppendEdit(docID, "main", singleInsert("a"), noCursors, noCursors, "main"); err != nil {
		t.Fatalf("AppendEdit: %v", err)
	}
	if _, err := s.CreateSnapshot(docID, "a", "local", 0); err != nil {
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

// TestSchemaConstraints verifies the integrity constraints the rebuilt schema
// adds actually reject bad rows — FK enforcement, the is_undo_stop CHECK, and the
// seq >= 0 CHECK. Each raw INSERT/UPDATE MUST error.
func TestSchemaConstraints(t *testing.T) {
	s := NewTestStore(t)
	docID := testDoc(t, s)
	const at = "2026-01-01T00:00:00Z"

	// FK: an event for a non-existent document is rejected (proves foreign_keys=ON).
	if _, err := s.perm.Exec(
		`INSERT INTO events(doc_id, surface, kind, is_undo_stop, at) VALUES(?,?,?,?,?)`,
		999999, "main", "edit", 1, at,
	); err == nil {
		t.Error("FK not enforced: event with bad doc_id was accepted")
	}

	// CHECK is_undo_stop IN (0,1): value 2 is rejected.
	if _, err := s.perm.Exec(
		`INSERT INTO events(doc_id, surface, kind, is_undo_stop, at) VALUES(?,?,?,?,?)`,
		docID, "main", "edit", 2, at,
	); err == nil {
		t.Error("CHECK not enforced: is_undo_stop=2 was accepted")
	}

	// CHECK seq >= 0 on snapshots: a negative seq is rejected (needs a real blob
	// so the blob_hash FK is satisfied and only the seq CHECK can fail).
	hash, err := s.PutBlob("x")
	if err != nil {
		t.Fatalf("PutBlob: %v", err)
	}
	if _, err := s.perm.Exec(
		`INSERT INTO snapshots(doc_id, blob_hash, source, seq, created_at) VALUES(?,?,?,?,?)`,
		docID, hash, "local", -1, at,
	); err == nil {
		t.Error("CHECK not enforced: snapshot with seq=-1 was accepted")
	}

	// CHECK current_seq >= 0 on documents.
	if _, err := s.perm.Exec(`UPDATE documents SET current_seq=-1 WHERE id=?`, docID); err == nil {
		t.Error("CHECK not enforced: documents.current_seq=-1 was accepted")
	}
}
