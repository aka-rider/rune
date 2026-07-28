package docstate

import (
	"database/sql"
	"path/filepath"
	"testing"

	"rune/pkg/editor/buffer"
	"rune/pkg/vfs"
)

// TestProbe_CrashSim_HashEqualityAutoAdopts simulates a crash between
// Materialize's atomic swap (step 3-4, which physically succeeds) and its
// ack tx (step 5, which never commits — the process dies first). On
// relaunch, the NEW session must discover that the fresh disk hash already
// equals the crashed session's dangling journal-head reconstruction and
// adopt it directly (Clean) rather than report Diverged — closing the WP4
// step-5 atomicity gap without ever training the user to press [S]ave-
// anyway on a crash that lost nothing.
//
// v10 (session-scoping): a process restart is now always a NEW session
// (Store.sessionID), so the self-heal that used to be Probe's own
// AdoptEqual auto-adopt is now reached through Load's cross-session
// inheritance decision (findInheritableDraft) instead: the crashed
// session's journal (postCrashContent) hash-equals what's physically on
// disk (the swap already landed it), so findInheritableDraft correctly
// decides there is nothing left to bridge, and an ordinary first-load
// adoption of disk content lands it on Clean directly — the exact same
// USER-VISIBLE outcome (no false Diverged prompt after a harmless crash),
// reached via the mechanism that actually fires now that two Store
// constructions are two different sessions.
//
// This drives findInheritableDraft/CreateSnapshot/recordAdoption directly
// (Load's own !hasHistory steps, white-box) against the ALREADY-KNOWN docID,
// rather than through a fresh path-based s2.Load(path): Materialize's own
// Exchange call (used below to simulate the physical swap, exactly like
// real Materialize step 3-4) deliberately churns the file's inode — by
// design, matching real disk — and commitSave's post-swap re-Bind (which
// re-syncs documents.inode to that new identity) is EXACTLY the step this
// test skips to model "crash before the ack ever ran". A fresh path-based
// Load after that point would hit OpenPath's own "new inode at this path"
// re-bind path and orphan docID onto a second row — a real, but SEPARATE
// and pre-existing, gap in inode-rebinding-after-an-ack-less-crash,
// orthogonal to the session-scoping this plan fixes and out of scope here.
func TestProbe_CrashSim_HashEqualityAutoAdopts(t *testing.T) {
	dir := t.TempDir()
	const relPath = "note.md"
	path := filepath.Join(dir, relPath)

	mem := vfs.NewMem()
	if err := mem.WriteFile(path, []byte("original"), 0o644); err != nil {
		t.Fatalf("seed file: %v", err)
	}

	// Session 1: load, then simulate Materialize up to (but not including)
	// the ack tx — the physical swap succeeds, but the crash happens before
	// observation(save)/saved_obs/Bind ever commit.
	dbPath := filepath.Join(dir, "rune.db")
	s1 := newStoreAtPath(t, dbPath)
	s1.UseFS(mem)

	loaded, err := s1.Load(path)
	if err != nil {
		t.Fatalf("Load: %v", err)
	}
	docID := loaded.DocID

	const postCrashContent = "journal-head content"
	// Journal the edit BEFORE the crash — this is what the buffer/journal
	// already reflects; only the disk-side ack never made it.
	edits := []buffer.AppliedEdit{{Start: 0, End: len("original"), Deleted: "original", Insert: postCrashContent}}
	if _, err := s1.AppendEdit(docID, edits, nil, nil); err != nil {
		t.Fatalf("AppendEdit: %v", err)
	}

	// Physically perform the swap (steps 3-4 of Materialize), WITHOUT ever
	// calling Materialize/commitSave — i.e. the crash lands exactly between
	// the swap and the ack tx.
	temp := path + ".crashtemp"
	if err := mem.WriteFile(temp, []byte(postCrashContent), 0o644); err != nil {
		t.Fatalf("write temp: %v", err)
	}
	if err := mem.Exchange(temp, path); err != nil {
		t.Fatalf("exchange: %v", err)
	}
	if err := s1.Close(); err != nil {
		t.Fatalf("close session 1: %v", err)
	}

	// Session 2: "relaunch" — a genuinely NEW session (v10). The physical
	// disk (mem) is the SAME; only the store handle is reopened, mirroring a
	// real crash where the .md file survives but rune.db's last transaction
	// for this save never committed. s2 shares this test's own pid with s1
	// (both are the SAME os process), so s1's session must be forced "dead"
	// here to model what a real second process's crash would present as —
	// s1's pid simply no longer existing.
	s2 := newStoreAtPath(t, dbPath)
	defer s2.Close()
	s2.UseFS(mem)
	s2.SetLivenessCheck(func(pid int, startedAt string) bool { return false })

	// Drive Load's own !hasHistory steps directly against the KNOWN docID
	// (see the test's doc comment for why this avoids a fresh path-based
	// Load call here).
	diskBytes, err := mem.ReadFile(path)
	if err != nil {
		t.Fatalf("read post-swap disk content: %v", err)
	}
	diskContent := string(diskBytes)
	diskHash := hashBytes(diskBytes)
	if diskContent != postCrashContent {
		t.Fatalf("post-swap disk content = %q, want %q", diskContent, postCrashContent)
	}

	draft, inheriting, err := s2.findInheritableDraft(docID, diskContent, diskHash)
	if err != nil {
		t.Fatalf("findInheritableDraft: %v", err)
	}
	if inheriting {
		t.Fatalf("findInheritableDraft: inheriting=true, want false (disk already hash-equals the crashed session's draft — nothing left to bridge)")
	}
	if draft != postCrashContent {
		t.Fatalf("findInheritableDraft: draft = %q, want %q", draft, postCrashContent)
	}
	if _, err := s2.CreateSnapshot(docID, diskContent, 0); err != nil {
		t.Fatalf("CreateSnapshot: %v", err)
	}
	if _, err := s2.recordAdoption(docID, diskHash, int64(len(diskBytes)), "", sql.NullInt64{}, sql.NullInt64{}, sql.NullInt64{}, "load", 0); err != nil {
		t.Fatalf("recordAdoption: %v", err)
	}

	recovered, err := s2.RecoverDocument(docID)
	if err != nil {
		t.Fatalf("RecoverDocument: %v", err)
	}
	if recovered != postCrashContent {
		t.Fatalf("RecoverDocument after crash-recovery adopt = %q, want %q", recovered, postCrashContent)
	}
	syncState, err := s2.Sync(docID)
	if err != nil {
		t.Fatalf("Sync: %v", err)
	}
	if syncState.Kind != SyncClean {
		t.Fatalf("Sync after crash-recovery adopt: Kind = %v, want SyncClean; Ours=%+v Theirs=%+v Ancestor=%+v",
			syncState.Kind, syncState.Ours, syncState.Theirs, syncState.Ancestor)
	}

	// The adoption must have actually moved saved_obs, not just reported
	// Clean transiently.
	savedObs, hasSaved, err := s2.SavedObs(docID)
	if err != nil || !hasSaved {
		t.Fatalf("SavedObs after crash-recovery adopt: hasSaved=%v err=%v", hasSaved, err)
	}
	if savedObs.BlobHash != hashBytes([]byte(postCrashContent)) {
		t.Fatalf("saved_obs hash after crash-recovery adopt = %q, want hash of %q", savedObs.BlobHash, postCrashContent)
	}

	// A subsequent idle Probe confirms the healed state is stable — no
	// spurious re-divergence from re-reading the same disk bytes.
	state, err := s2.Probe(docID)
	if err != nil {
		t.Fatalf("Probe: %v", err)
	}
	if state.Kind != SyncClean {
		t.Fatalf("Probe after crash-recovery: Kind = %v, want SyncClean; Ours=%+v Theirs=%+v Ancestor=%+v",
			state.Kind, state.Ours, state.Theirs, state.Ancestor)
	}
}
