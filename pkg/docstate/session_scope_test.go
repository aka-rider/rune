package docstate

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"rune/pkg/editor/buffer"
	"rune/pkg/merge"
)

// ---- test helpers for simulating a SECOND (often dead) session without a
// real second OS process — see the plan's "Testability" note: two
// independent *Store handles opened against the same real directory (e.g.
// via OpenAt) naturally get two distinct sessionIDs for free; SIMULATING a
// DEAD session needs a seam, which these helpers provide via a direct
// test-only insert into sessions/snapshots (white-box, same package). ------

// seedSession inserts a raw sessions row for a session THIS test controls
// directly (never through Store's own establishSession) — pid/startedAt let
// the test decide, via the REAL isProcessAlive, whether it reads as alive
// (e.g. this test process's own real pid+starttime) or dead (an implausibly
// large pid, e.g. 999999999, that virtually never corresponds to a real
// running process on any platform).
func seedSession(t *testing.T, s *Store, pid int64, startedAt string) int64 {
	t.Helper()
	res, err := s.perm.Exec(
		`INSERT INTO sessions(pid, proc_started_at, opened_at) VALUES(?,?,?)`,
		pid, startedAt, time.Now().UTC().Format(time.RFC3339Nano),
	)
	if err != nil {
		t.Fatalf("seedSession: %v", err)
	}
	id, err := res.LastInsertId()
	if err != nil {
		t.Fatalf("seedSession: last insert id: %v", err)
	}
	return id
}

// seedSessionSnapshot plants a recovery anchor for docID under sessionID —
// exactly what recoverAt(docID, sessionID) needs to reconstruct content
// "as seen by" a session this test controls directly, without ever routing
// through that session's own (nonexistent) Store handle.
func seedSessionSnapshot(t *testing.T, s *Store, sessionID, docID int64, content string, seq int64) {
	t.Helper()
	hash, err := s.PutBlob(content)
	if err != nil {
		t.Fatalf("seedSessionSnapshot: PutBlob: %v", err)
	}
	if _, err := s.perm.Exec(
		`INSERT INTO snapshots(doc_id, session_id, blob_hash, seq, created_at) VALUES(?,?,?,?,?)`,
		docID, sessionID, hash, seq, time.Now().UTC().Format(time.RFC3339Nano),
	); err != nil {
		t.Fatalf("seedSessionSnapshot: insert: %v", err)
	}
}

// realAlivePid/realAliveStartedAt let a test seed a session that the REAL
// (unmocked) isProcessAlive genuinely reports as alive — this test process's
// own pid and start time — mirroring the plan's own suggested companion
// case ("seed the row with the test process's own real pid+starttime").
func realAlivePid(t *testing.T) (pid int64, startedAt string) {
	t.Helper()
	p := os.Getpid()
	sa, _ := processStartedAt(p) // ok ignored: "" is still a valid (alive-by-existence) value on platforms without start-time support
	return int64(p), sa
}

// ---- Verification 1+2: the reported bug, and its fix ----------------------

// TestTwoSessions_SameDoc_IsolatedUndoRedoAndDivergence is the plan's
// Verification items 1+2(a)+2(b) in one scenario: two independent *Store
// handles (two rune windows) on the SAME real file/docID. It first proves
// undo/redo isolation (root-cause #1/#2: a session's own undo/redo can
// never see or be corrupted by a DIFFERENT session's keystrokes to the same
// doc), then reproduces the EXACT ordering that broke Sync pre-fix — B
// saves, A journals one MORE edit after B's save lands, then A saves — and
// asserts Sync now correctly reports Diverged (not BufferAhead, the B1
// blocker) and that Materialize's own CAS check independently refuses the
// clobber (defense in depth) even if a caller ignored Sync's classification.
func TestTwoSessions_SameDoc_IsolatedUndoRedoAndDivergence(t *testing.T) {
	dir := t.TempDir()

	a, warn, err := OpenAt(dir)
	if err != nil {
		t.Fatalf("OpenAt a: %v", err)
	}
	defer a.Close()
	if warn != "" {
		t.Fatalf("unexpected warning: %q", warn)
	}
	b, warn, err := OpenAt(dir)
	if err != nil {
		t.Fatalf("OpenAt b: %v", err)
	}
	defer b.Close()
	if warn != "" {
		t.Fatalf("unexpected warning: %q", warn)
	}
	if a.sessionID == b.sessionID {
		t.Fatalf("two independent OpenAt calls got the SAME session id (%d) — sessions are not actually distinct", a.sessionID)
	}

	path := filepath.Join(dir, "shared.md")
	if err := a.fsys().WriteFile(path, []byte("original\n"), 0o644); err != nil {
		t.Fatalf("seed file: %v", err)
	}

	loadA, err := a.Load(path)
	if err != nil {
		t.Fatalf("a.Load: %v", err)
	}
	loadB, err := b.Load(path)
	if err != nil {
		t.Fatalf("b.Load: %v", err)
	}
	if loadA.DocID != loadB.DocID {
		t.Fatalf("both processes must resolve the SAME docID, got %d vs %d", loadA.DocID, loadB.DocID)
	}
	docID := loadA.DocID

	// Both edit — non-coalescing (multi-char) inserts so the 300ms
	// coalescing window (a SEPARATE root-cause item) never interferes with
	// isolating THIS assertion.
	if _, err := a.AppendEdit(docID, wordInsert("A-edit"), noCursors, noCursors); err != nil {
		t.Fatalf("a.AppendEdit: %v", err)
	}
	if _, err := b.AppendEdit(docID, wordInsert("B-edit"), noCursors, noCursors); err != nil {
		t.Fatalf("b.AppendEdit: %v", err)
	}

	// --- Isolation (root cause #1/#2): each session's own reconstruction
	// reflects ONLY its own edit.
	aContent, err := a.RecoverDocument(docID)
	if err != nil {
		t.Fatalf("a.RecoverDocument: %v", err)
	}
	if !strings.Contains(aContent, "A-edit") || strings.Contains(aContent, "B-edit") {
		t.Fatalf("a.RecoverDocument = %q — must contain ONLY A's edit", aContent)
	}
	bContent, err := b.RecoverDocument(docID)
	if err != nil {
		t.Fatalf("b.RecoverDocument: %v", err)
	}
	if !strings.Contains(bContent, "B-edit") || strings.Contains(bContent, "A-edit") {
		t.Fatalf("b.RecoverDocument = %q — must contain ONLY B's edit", bContent)
	}

	// --- Undo isolation: A's own UndoPeek can only ever see A's own event.
	step, ok, err := a.UndoPeek(docID)
	if err != nil {
		t.Fatalf("a.UndoPeek: %v", err)
	}
	if !ok {
		t.Fatal("a.UndoPeek: ok=false, want A's own edit available to undo")
	}
	for _, e := range step.Edits {
		if strings.Contains(e.Insert, "B-edit") {
			t.Fatalf("a.UndoPeek exposed B's edit: %+v", step)
		}
	}
	if err := a.MoveUndoPos(docID, step.NewPos); err != nil {
		t.Fatalf("a.MoveUndoPos: %v", err)
	}
	aAfterUndo, err := a.RecoverDocument(docID)
	if err != nil {
		t.Fatalf("a.RecoverDocument after undo: %v", err)
	}
	if aAfterUndo != "original\n" {
		t.Fatalf("a after undoing its OWN only edit = %q, want the untouched original (never B's row)", aAfterUndo)
	}
	if _, ok2, err := a.UndoPeek(docID); err != nil {
		t.Fatalf("a.UndoPeek (second): %v", err)
	} else if ok2 {
		t.Fatal("a has a SECOND undo step available — it must have reached into B's event")
	}
	// Redo A's edit back so the divergence scenario below proceeds with A
	// dirty again.
	stepR, okR, err := a.RedoPeek(docID)
	if err != nil || !okR {
		t.Fatalf("a.RedoPeek: ok=%v err=%v", okR, err)
	}
	if err := a.MoveUndoPos(docID, stepR.NewPos); err != nil {
		t.Fatalf("a.MoveUndoPos (redo): %v", err)
	}

	// --- B saves.
	bExpect, ok, err := b.SavedObs(docID)
	if err != nil || !ok {
		t.Fatalf("b.SavedObs: ok=%v err=%v", ok, err)
	}
	bSaveContent, err := b.Content(docID)
	if err != nil {
		t.Fatalf("b.Content: %v", err)
	}
	bSeq, err := b.CurrentSeq(docID)
	if err != nil {
		t.Fatalf("b.CurrentSeq: %v", err)
	}
	bRes, err := b.Materialize(docID, path, bSaveContent, bExpect.ID, bSeq, false)
	if err != nil {
		t.Fatalf("b.Materialize: %v", err)
	}
	if !bRes.Committed {
		t.Fatalf("b's first save should commit cleanly: %+v", bRes)
	}

	// --- THE ordering that matters: A journals ONE MORE edit AFTER B's save
	// landed (this is what defeated ancestorAt's self-exclusion pre-fix).
	if _, err := a.AppendEdit(docID, wordInsert("A-more"), noCursors, noCursors); err != nil {
		t.Fatalf("a.AppendEdit (post-B-save): %v", err)
	}

	// --- THE B1 FIX ASSERTION: A's Sync must report Diverged, never
	// BufferAhead.
	aSync, err := a.Sync(docID)
	if err != nil {
		t.Fatalf("a.Sync: %v", err)
	}
	if aSync.Kind != SyncDiverged {
		t.Fatalf("a.Sync.Kind = %v, want SyncDiverged (B1 fix) — Ours=%+v Theirs=%+v Ancestor=%+v",
			aSync.Kind, aSync.Ours, aSync.Theirs, aSync.Ancestor)
	}

	// --- Defense in depth: even calling Materialize directly (bypassing
	// the workspace's own vetSave gate, which would have refused before
	// ever reaching here) must independently refuse — A's own saved_obs
	// still reflects the ORIGINAL load, not B's save, so live disk no
	// longer matches A's CAS expectation.
	aExpect, ok, err := a.SavedObs(docID)
	if err != nil || !ok {
		t.Fatalf("a.SavedObs: ok=%v err=%v", ok, err)
	}
	aSaveContent, err := a.Content(docID)
	if err != nil {
		t.Fatalf("a.Content: %v", err)
	}
	aSeq, err := a.CurrentSeq(docID)
	if err != nil {
		t.Fatalf("a.CurrentSeq: %v", err)
	}
	aRes, err := a.Materialize(docID, path, aSaveContent, aExpect.ID, aSeq, false)
	if err != nil {
		t.Fatalf("a.Materialize: %v", err)
	}
	if aRes.Committed {
		t.Fatalf("a's save COMMITTED despite Diverged — CAS defense-in-depth failed to refuse the clobber: %+v", aRes)
	}

	diskNow, err := a.fsys().ReadFile(path)
	if err != nil {
		t.Fatalf("read disk: %v", err)
	}
	if string(diskNow) != bSaveContent {
		t.Fatalf("disk content changed despite A's save being refused: got %q, want B's still-intact save %q", diskNow, bSaveContent)
	}
}

// TestTwoSessions_MergeResolution_UsesTruePreDivergenceAncestor is the
// plan's Verification 2(c) — the actual B1 hole the classification alone
// doesn't prove closed: when A and B diverge from a common baseline and B
// saves first, the [M]erge path (merge.MergeHunks, driven directly here as
// the plan specifies) must run against the TRUE common ancestor — never B's
// post-save content — so B's change survives as a reconcilable hunk instead
// of being silently discarded.
func TestTwoSessions_MergeResolution_UsesTruePreDivergenceAncestor(t *testing.T) {
	dir := t.TempDir()
	a, _, err := OpenAt(dir)
	if err != nil {
		t.Fatalf("OpenAt a: %v", err)
	}
	defer a.Close()
	b, _, err := OpenAt(dir)
	if err != nil {
		t.Fatalf("OpenAt b: %v", err)
	}
	defer b.Close()

	path := filepath.Join(dir, "shared.md")
	const baseline = "line one\nline two\nline three\n"
	if err := a.fsys().WriteFile(path, []byte(baseline), 0o644); err != nil {
		t.Fatalf("seed file: %v", err)
	}

	loadA, err := a.Load(path)
	if err != nil {
		t.Fatalf("a.Load: %v", err)
	}
	if _, err := b.Load(path); err != nil {
		t.Fatalf("b.Load: %v", err)
	}
	docID := loadA.DocID

	// A changes line one; B changes line three — non-overlapping, so a real
	// 3-way merge cleanly reconciles both without a conflict block.
	aFirstEdit := "A CHANGED LINE ONE\nline two\nline three\n"
	if _, err := a.AppendEdit(docID,
		[]buffer.AppliedEdit{{Start: 0, End: len(baseline), Deleted: baseline, Insert: aFirstEdit}},
		nil, nil); err != nil {
		t.Fatalf("a.AppendEdit: %v", err)
	}
	bEdit := "line one\nline two\nB CHANGED LINE THREE\n"
	if _, err := b.AppendEdit(docID,
		[]buffer.AppliedEdit{{Start: 0, End: len(baseline), Deleted: baseline, Insert: bEdit}},
		nil, nil); err != nil {
		t.Fatalf("b.AppendEdit: %v", err)
	}

	// B saves.
	bExpect, ok, err := b.SavedObs(docID)
	if err != nil || !ok {
		t.Fatalf("b.SavedObs: ok=%v err=%v", ok, err)
	}
	bSeq, err := b.CurrentSeq(docID)
	if err != nil {
		t.Fatalf("b.CurrentSeq: %v", err)
	}
	bRes, err := b.Materialize(docID, path, bEdit, bExpect.ID, bSeq, false)
	if err != nil || !bRes.Committed {
		t.Fatalf("b.Materialize: res=%+v err=%v", bRes, err)
	}

	// A journals one more edit AFTER B's save (the ordering that matters —
	// see the sibling test above).
	oursFinal := aFirstEdit + "A-second-edit\n"
	if _, err := a.AppendEdit(docID,
		[]buffer.AppliedEdit{{Start: len(aFirstEdit), End: len(aFirstEdit), Deleted: "", Insert: "A-second-edit\n"}},
		nil, nil); err != nil {
		t.Fatalf("a.AppendEdit (post-B-save): %v", err)
	}

	aSync, err := a.Sync(docID)
	if err != nil {
		t.Fatalf("a.Sync: %v", err)
	}
	if aSync.Kind != SyncDiverged {
		t.Fatalf("a.Sync.Kind = %v, want SyncDiverged", aSync.Kind)
	}
	if !aSync.Ancestor.Valid {
		t.Fatal("a.Sync.Ancestor is not valid — no ancestor to merge against")
	}

	ancestorContent, err := a.GetBlob(aSync.Ancestor.Hash)
	if err != nil {
		t.Fatalf("GetBlob(ancestor): %v", err)
	}
	if ancestorContent != baseline {
		t.Fatalf("Sync.Ancestor content = %q, want the TRUE pre-divergence baseline %q (not B's post-save content)", ancestorContent, baseline)
	}
	theirsContent, err := a.GetBlob(aSync.Theirs.Hash)
	if err != nil {
		t.Fatalf("GetBlob(theirs): %v", err)
	}
	if theirsContent != bEdit {
		t.Fatalf("Sync.Theirs content = %q, want B's saved content %q", theirsContent, bEdit)
	}

	oursContent, err := a.RecoverDocument(docID)
	if err != nil {
		t.Fatalf("a.RecoverDocument: %v", err)
	}
	if oursContent != oursFinal {
		t.Fatalf("a.RecoverDocument = %q, want %q", oursContent, oursFinal)
	}

	// Drive the ACTUAL merge — the part classification alone doesn't prove.
	hunks, err := merge.MergeHunks([]byte(ancestorContent), []byte(oursContent), []byte(theirsContent))
	if err != nil {
		t.Fatalf("merge.MergeHunks: %v", err)
	}

	var merged strings.Builder
	foundOurs, foundTheirs := false, false
	for _, h := range hunks {
		switch h.Kind {
		case merge.HunkClean:
			merged.Write(h.AutoBytes)
			if strings.Contains(string(h.AutoBytes), "A CHANGED LINE ONE") {
				foundOurs = true
			}
			if strings.Contains(string(h.AutoBytes), "B CHANGED LINE THREE") {
				foundTheirs = true
			}
		case merge.HunkConflict:
			merged.Write(h.OursBytes)
			if strings.Contains(string(h.OursBytes), "A CHANGED LINE ONE") {
				foundOurs = true
			}
			if strings.Contains(string(h.TheirsBytes), "B CHANGED LINE THREE") {
				foundTheirs = true
			}
		}
	}
	if !foundTheirs {
		t.Fatalf("merge SILENTLY DROPPED B's change (the B1 hole) — no hunk contains it. hunks=%+v merged=%q", hunks, merged.String())
	}
	if !foundOurs {
		t.Fatalf("merge lost A's own change: hunks=%+v merged=%q", hunks, merged.String())
	}
}

// ---- Verification 3: crash recovery across sessions ------------------------

// TestLoad_CrossSession_DeadSessionDraftInherited seeds a sessions row with a
// pid that does not correspond to any currently-running process, plus a
// journaled (but never materialized) anchor for a docID — then opens that
// SAME docID via a NEW session's Load and asserts it inherits the dead
// session's unsaved content, lands on SyncBufferAhead (dirty, safe to save,
// never a false Diverged), and that an immediate save against the freshly
// adopted CAS baseline succeeds.
func TestLoad_CrossSession_DeadSessionDraftInherited(t *testing.T) {
	s, mem := newMatTestStore(t)
	const path = "/note.md"
	const diskContent = "disk content, never touched by the crashed session's draft\n"
	if err := mem.WriteFile(path, []byte(diskContent), 0o644); err != nil {
		t.Fatalf("seed file: %v", err)
	}

	ref, err := s.OpenPath(path)
	if err != nil {
		t.Fatalf("OpenPath: %v", err)
	}
	docID := ref.ID

	deadSessionID := seedSession(t, s, 999999999, "")
	const draftContent = "unsaved draft content the crashed session never saved\n"
	seedSessionSnapshot(t, s, deadSessionID, docID, draftContent, 0)

	loaded, err := s.Load(path)
	if err != nil {
		t.Fatalf("Load: %v", err)
	}
	if loaded.DocID != docID {
		t.Fatalf("Load resolved a different docID: %d != %d", loaded.DocID, docID)
	}
	if loaded.Recovered != draftContent {
		t.Fatalf("Load.Recovered = %q, want the dead session's draft %q", loaded.Recovered, draftContent)
	}
	if loaded.Sync.Kind != SyncBufferAhead {
		t.Fatalf("Load.Sync.Kind = %v, want SyncBufferAhead (dirty, safe to save); Ours=%+v Theirs=%+v Ancestor=%+v",
			loaded.Sync.Kind, loaded.Sync.Ours, loaded.Sync.Theirs, loaded.Sync.Ancestor)
	}

	expect, ok, err := s.SavedObs(docID)
	if err != nil || !ok {
		t.Fatalf("SavedObs: ok=%v err=%v", ok, err)
	}
	seq, err := s.CurrentSeq(docID)
	if err != nil {
		t.Fatalf("CurrentSeq: %v", err)
	}
	res, err := s.Materialize(docID, path, draftContent, expect.ID, seq, false)
	if err != nil {
		t.Fatalf("Materialize: %v", err)
	}
	if !res.Committed {
		t.Fatalf("immediate save of the inherited draft was refused: %+v", res)
	}
	finalDisk, err := mem.ReadFile(path)
	if err != nil {
		t.Fatalf("read final disk: %v", err)
	}
	if string(finalDisk) != draftContent {
		t.Fatalf("final disk content = %q, want the inherited draft %q", finalDisk, draftContent)
	}
}

// TestLoad_CrossSession_AliveSessionDraftNotInherited is the companion case:
// seeding the row with the TEST PROCESS'S OWN real pid+starttime (alive)
// must NOT be inherited — the new session starts fresh from disk instead,
// exactly like an ordinary first load.
func TestLoad_CrossSession_AliveSessionDraftNotInherited(t *testing.T) {
	s, mem := newMatTestStore(t)
	const path = "/note.md"
	const diskContent = "plain disk content\n"
	if err := mem.WriteFile(path, []byte(diskContent), 0o644); err != nil {
		t.Fatalf("seed file: %v", err)
	}

	ref, err := s.OpenPath(path)
	if err != nil {
		t.Fatalf("OpenPath: %v", err)
	}
	docID := ref.ID

	alivePid, aliveStartedAt := realAlivePid(t)
	aliveSessionID := seedSession(t, s, alivePid, aliveStartedAt)
	const privateDraft = "a still-live window's private unsaved draft\n"
	seedSessionSnapshot(t, s, aliveSessionID, docID, privateDraft, 0)

	loaded, err := s.Load(path)
	if err != nil {
		t.Fatalf("Load: %v", err)
	}
	if loaded.Recovered != diskContent {
		t.Fatalf("Load leaked an ALIVE session's private draft: Recovered = %q, want plain disk content %q", loaded.Recovered, diskContent)
	}
	if loaded.Sync.Kind != SyncClean {
		t.Fatalf("Load.Sync.Kind = %v, want SyncClean (fresh load of disk content)", loaded.Sync.Kind)
	}
}

// TestLoad_CrossSession_StaleSaveNotInheritedOverNewerExternalEdit is the
// necessary counterpart to TestLoad_CrossSession_DeadSessionDraftInherited
// (review finding, post-worker): a dead session that PROPERLY SAVED before
// quitting cleanly (no crash — the ordinary case, not just the crash case)
// reconstructs to EXACTLY what it itself last agreed was on disk. If disk is
// LATER changed by something entirely unrelated (an external tool, another
// rune session that already came and went) before a new session opens the
// file, that reconstruction is not unsaved work — it is a stale mirror of a
// PAST disk state, and must never be bridged in as "our" draft: doing so
// classifies the new session as BufferAhead against content strictly OLDER
// than current disk, and an immediate save would silently discard the newer
// disk content with NO conflict guard ever raised (Materialize's CAS check
// passes cleanly — the new session's own adoption is anchored on, and disk
// has not moved since, its own fresh load). Empirically reproduced against
// the pre-fix code here before landing the fix (findInheritableDraft's
// dead-session baseline check, load.go).
func TestLoad_CrossSession_StaleSaveNotInheritedOverNewerExternalEdit(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "note.md")

	x, _, err := OpenAt(dir)
	if err != nil {
		t.Fatalf("OpenAt x: %v", err)
	}
	if err := x.fsys().WriteFile(path, []byte("original\n"), 0o644); err != nil {
		t.Fatalf("seed file: %v", err)
	}
	loadX, err := x.Load(path)
	if err != nil {
		t.Fatalf("x.Load: %v", err)
	}
	docID := loadX.DocID
	if _, err := x.AppendEdit(docID, wordInsert("X-edit"), noCursors, noCursors); err != nil {
		t.Fatalf("x.AppendEdit: %v", err)
	}
	xExpect, ok, err := x.SavedObs(docID)
	if err != nil || !ok {
		t.Fatalf("x.SavedObs: ok=%v err=%v", ok, err)
	}
	xContent, err := x.Content(docID)
	if err != nil {
		t.Fatalf("x.Content: %v", err)
	}
	xSeq, err := x.CurrentSeq(docID)
	if err != nil {
		t.Fatalf("x.CurrentSeq: %v", err)
	}
	xRes, err := x.Materialize(docID, path, xContent, xExpect.ID, xSeq, false)
	if err != nil || !xRes.Committed {
		t.Fatalf("x.Materialize: res=%+v err=%v", xRes, err)
	}
	if err := x.Close(); err != nil {
		t.Fatalf("x.Close: %v", err)
	}

	// X's process has genuinely ended (a normal quit, no crash). Something
	// ELSE now changes the file — a vim save, a git checkout, a formatter —
	// completely independent of X and of any rune session.
	const externalContent = "externally edited after X quit cleanly\n"
	if err := os.WriteFile(path, []byte(externalContent), 0o644); err != nil {
		t.Fatalf("external write: %v", err)
	}

	y, _, err := OpenAt(dir)
	if err != nil {
		t.Fatalf("OpenAt y: %v", err)
	}
	defer y.Close()
	y.SetLivenessCheck(func(pid int, startedAt string) bool { return false }) // X is dead

	loaded, err := y.Load(path)
	if err != nil {
		t.Fatalf("y.Load: %v", err)
	}
	if loaded.Recovered != externalContent {
		t.Fatalf("Load.Recovered = %q — resurrected X's stale saved content instead of the newer external edit %q", loaded.Recovered, externalContent)
	}
	if loaded.Sync.Kind != SyncClean {
		t.Fatalf("Load.Sync.Kind = %v, want SyncClean (fresh load of current disk, nothing stale bridged in); Ours=%+v Theirs=%+v Ancestor=%+v",
			loaded.Sync.Kind, loaded.Sync.Ours, loaded.Sync.Theirs, loaded.Sync.Ancestor)
	}

	// Belt and suspenders: an immediate save of whatever Y now shows must
	// never physically change disk away from the external edit — proving
	// the fix, not just the classification.
	yExpect, ok, err := y.SavedObs(docID)
	if err != nil || !ok {
		t.Fatalf("y.SavedObs: ok=%v err=%v", ok, err)
	}
	ySeq, err := y.CurrentSeq(docID)
	if err != nil {
		t.Fatalf("y.CurrentSeq: %v", err)
	}
	// Nothing is dirty (SyncClean), so an ordinary workspace would never even
	// attempt this save — but Materialize is checked directly anyway, as
	// defense in depth matching the sibling isolation test above.
	yRes, err := y.Materialize(docID, path, loaded.Recovered, yExpect.ID, ySeq, false)
	if err != nil {
		t.Fatalf("y.Materialize: %v", err)
	}
	finalDisk, err := y.fsys().ReadFile(path)
	if err != nil {
		t.Fatalf("read final disk: %v", err)
	}
	if string(finalDisk) != externalContent {
		t.Fatalf("CATASTROPHIC: external edit destroyed — final disk = %q, want %q (materialize result: %+v)", finalDisk, externalContent, yRes)
	}
}

// TestRecoverAcrossSessions_ScratchDoc_DeadSessionRecovered is the B2
// companion for untitled/scratch documents (no disk fallback at all): a
// dead session's non-empty draft must be recoverable.
func TestRecoverAcrossSessions_ScratchDoc_DeadSessionRecovered(t *testing.T) {
	s := NewTestStore(t)
	ref, err := s.CreateScratch("")
	if err != nil {
		t.Fatalf("CreateScratch: %v", err)
	}
	docID := ref.ID

	deadSessionID := seedSession(t, s, 999999999, "")
	const draft = "a crashed session's untitled draft\n"
	seedSessionSnapshot(t, s, deadSessionID, docID, draft, 0)

	content, found, err := s.RecoverAcrossSessions(docID)
	if err != nil {
		t.Fatalf("RecoverAcrossSessions: %v", err)
	}
	if !found {
		t.Fatal("RecoverAcrossSessions: found=false, want the dead session's draft")
	}
	if content != draft {
		t.Fatalf("RecoverAcrossSessions content = %q, want %q", content, draft)
	}
}

// TestRecoverAcrossSessions_ScratchDoc_AliveSessionNotOffered mirrors the
// dead case: an alive session's untitled draft must stay completely
// private — found=false, never a stolen/empty peek.
func TestRecoverAcrossSessions_ScratchDoc_AliveSessionNotOffered(t *testing.T) {
	s := NewTestStore(t)
	ref, err := s.CreateScratch("")
	if err != nil {
		t.Fatalf("CreateScratch: %v", err)
	}
	docID := ref.ID

	alivePid, aliveStartedAt := realAlivePid(t)
	aliveSessionID := seedSession(t, s, alivePid, aliveStartedAt)
	seedSessionSnapshot(t, s, aliveSessionID, docID, "still being typed elsewhere\n", 0)

	content, found, err := s.RecoverAcrossSessions(docID)
	if err != nil {
		t.Fatalf("RecoverAcrossSessions: %v", err)
	}
	if found {
		t.Fatalf("RecoverAcrossSessions leaked an ALIVE session's private draft: content=%q", content)
	}
}

// TestRecoverAcrossSessions_OwnSessionAlreadyHasHistory pins the ordinary
// (non-cross-session) case: once THIS session has touched docID itself,
// RecoverAcrossSessions must read its OWN history, never re-consult
// mostRecentSessionForDoc at all.
func TestRecoverAcrossSessions_OwnSessionAlreadyHasHistory(t *testing.T) {
	s := NewTestStore(t)
	docID := testDoc(t, s)
	if _, err := s.AppendEdit(docID, wordInsert("mine"), noCursors, noCursors); err != nil {
		t.Fatalf("AppendEdit: %v", err)
	}
	content, found, err := s.RecoverAcrossSessions(docID)
	if err != nil {
		t.Fatalf("RecoverAcrossSessions: %v", err)
	}
	if !found || content != "mine" {
		t.Fatalf("RecoverAcrossSessions = (%q, %v), want (\"mine\", true)", content, found)
	}
}

// ---- R1: dead-session reaper ------------------------------------------------

// TestReapDeadSessions_SupersededSessionIsReaped: a dead session's footprint
// is removed once some LATER session has journaled its own edit for the
// same docID (superseding it by seq — mostRecentSessionForDoc). The
// sessions row itself (a tombstone — see liveness.go) must survive
// (observations may still reference it); only its session_documents/events/
// snapshots footprint goes.
func TestReapDeadSessions_SupersededSessionIsReaped(t *testing.T) {
	s := NewTestStore(t)
	docID := testDoc(t, s)

	deadSessionID := seedSession(t, s, 999999999, "")
	seedSessionSnapshot(t, s, deadSessionID, docID, "dead draft\n", 0)

	reapableBefore, err := sessionIsReapable(s.perm, deadSessionID)
	if err != nil {
		t.Fatalf("sessionIsReapable (before): %v", err)
	}
	if reapableBefore {
		t.Fatal("dead session reapable=true before any supersession — it is still the only/most-recent toucher of docID")
	}

	// A DIFFERENT, live session (s itself) now builds on the doc — its own
	// AppendEdit lands at a higher seq, superseding the dead session.
	if _, err := s.AppendEdit(docID, wordInsert("s builds on it"), noCursors, noCursors); err != nil {
		t.Fatalf("AppendEdit: %v", err)
	}

	// Force EVERY session (including s's own) through the "dead" branch of
	// reapDeadSessions, to isolate and exercise ONLY the reapability safety
	// check (sessionIsReapable) — s's own session must survive regardless,
	// since it is now the most-recent toucher of docID.
	if err := reapDeadSessions(s.perm, func(pid int, startedAt string) bool { return false }); err != nil {
		t.Fatalf("reapDeadSessions: %v", err)
	}

	var deadRowExists bool
	if err := s.perm.QueryRow(`SELECT EXISTS(SELECT 1 FROM sessions WHERE id=?)`, deadSessionID).Scan(&deadRowExists); err != nil {
		t.Fatalf("query sessions: %v", err)
	}
	if !deadRowExists {
		t.Fatal("reaper deleted the sessions row itself — must only reap its session_documents/events/snapshots footprint (observations.session_id has no cascade)")
	}
	var footprint int
	if err := s.perm.QueryRow(
		`SELECT (SELECT COUNT(*) FROM snapshots WHERE session_id=?) + (SELECT COUNT(*) FROM events WHERE session_id=?)`,
		deadSessionID, deadSessionID,
	).Scan(&footprint); err != nil {
		t.Fatalf("query footprint: %v", err)
	}
	if footprint != 0 {
		t.Fatalf("dead session's footprint not reaped: %d rows remain", footprint)
	}

	// s's OWN (still most-recent) session must be completely unaffected.
	content, err := s.RecoverDocument(docID)
	if err != nil {
		t.Fatalf("s.RecoverDocument after reap: %v", err)
	}
	if !strings.Contains(content, "s builds on it") {
		t.Fatalf("s's own content was damaged by the reaper: %q", content)
	}
}

// TestReapDeadSessions_StillMostRecentIsNotReaped: a dead session that is
// STILL "the most recent" for some docID (nobody has inherited/superseded
// it yet) must NOT be reaped — its content stays recoverable afterward.
// Reaping it would destroy the exact unsaved content the next opener still
// needs to inherit (the falsifiable safety condition R1 names).
func TestReapDeadSessions_StillMostRecentIsNotReaped(t *testing.T) {
	s := NewTestStore(t)
	docID := testDoc(t, s)

	deadSessionID := seedSession(t, s, 999999999, "")
	const draft = "still the only content anyone has ever written for this doc\n"
	seedSessionSnapshot(t, s, deadSessionID, docID, draft, 0)

	if err := reapDeadSessions(s.perm, func(pid int, startedAt string) bool { return false }); err != nil {
		t.Fatalf("reapDeadSessions: %v", err)
	}

	var n int
	if err := s.perm.QueryRow(`SELECT COUNT(*) FROM snapshots WHERE session_id=?`, deadSessionID).Scan(&n); err != nil {
		t.Fatalf("query snapshots: %v", err)
	}
	if n == 0 {
		t.Fatal("dead-but-still-most-recent session's snapshot was reaped — its content is now unrecoverable")
	}

	content, found, err := s.RecoverAcrossSessions(docID)
	if err != nil {
		t.Fatalf("RecoverAcrossSessions after (non-)reap: %v", err)
	}
	if !found || content != draft {
		t.Fatalf("content no longer recoverable after reap decision: found=%v content=%q, want %q", found, content, draft)
	}
}

// TestReapDeadSessions_AliveSessionNeverReaped: an ALIVE session (this test
// process's own real pid) is never even considered for reaping, regardless
// of the reapability safety check — the liveness gate runs first.
func TestReapDeadSessions_AliveSessionNeverReaped(t *testing.T) {
	s := NewTestStore(t)
	docID := testDoc(t, s)
	alivePid, aliveStartedAt := realAlivePid(t)
	aliveSessionID := seedSession(t, s, alivePid, aliveStartedAt)
	seedSessionSnapshot(t, s, aliveSessionID, docID, "alive session content\n", 0)

	if err := reapDeadSessions(s.perm, isProcessAlive); err != nil {
		t.Fatalf("reapDeadSessions: %v", err)
	}

	var n int
	if err := s.perm.QueryRow(`SELECT COUNT(*) FROM snapshots WHERE session_id=?`, aliveSessionID).Scan(&n); err != nil {
		t.Fatalf("query snapshots: %v", err)
	}
	if n == 0 {
		t.Fatal("an ALIVE session's snapshot was reaped")
	}
}
