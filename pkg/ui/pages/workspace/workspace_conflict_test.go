package workspace

import (
	"os"
	"path/filepath"
	"testing"

	"rune/pkg/docstate"
	"rune/pkg/ui/components/footer"
	"rune/pkg/ui/components/opentabs"
	"rune/pkg/ui/pages/workspace/mergemode"
	"rune/pkg/vfs"
)

// ─────────────────────────────────────────────────────────────────────────────
// setupSaveConflict: a real, store-backed CAS-refused save (WP5 — replaces the
// pre-v4 size/mtime baseline + DB-anchor backstop with store.Materialize's own
// content-hash CAS check, Part III steps 1-2). Loads oursLoaded, writes
// theirsExternal to disk AFTER the load (so the save's pre-write hash check
// fails), attempts an interactive ⌘S, and asserts the resulting
// FileSaveErrorMsg{Conflict:true} correctly raised pendingConflict.
// ─────────────────────────────────────────────────────────────────────────────

func setupSaveConflict(t *testing.T, oursLoaded, theirsExternal string) (Model, string, int64) {
	t.Helper()
	dir := t.TempDir()
	path := filepath.Join(dir, "note.md")
	if err := os.WriteFile(path, []byte(oursLoaded), 0o644); err != nil {
		t.Fatal(err)
	}

	m := withStore(t, newTestWorkspace(t))
	m = m.WithFS(vfs.Disk{})
	m = loadFile(m, path, oursLoaded)
	docID := m.view.DocID()
	if docID == 0 {
		t.Skip("store not available — docID is 0")
	}
	m = focusEditor(m)

	// External change AFTER load — the save's unconditional pre-write hash
	// (Materialize step 1-2) will now refuse.
	if err := os.WriteFile(path, []byte(theirsExternal), 0o644); err != nil {
		t.Fatal(err)
	}

	m, saveCmd := m.startSave()
	if saveCmd == nil {
		t.Fatalf("setup: startSave returned nil cmd")
	}
	msg := saveCmd()
	m, _ = m.Update(msg)
	if !m.pendingConflict.active {
		t.Fatalf("setup: expected conflict guard raised from save; got msg=%#v", msg)
	}
	return m, path, docID
}

// ─────────────────────────────────────────────────────────────────────────────
// evictSave: content-hash CAS refuses to overwrite a diverged background tab
// (§1.4.7) — replaces the pre-v4 per-tab baseline + DB-anchor backstop tests.
// ─────────────────────────────────────────────────────────────────────────────

func TestEvictSave_DivergenceRefusesWrite(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "victim.md")
	const original = "original content"
	const external = "EXTERNAL — must not be clobbered"

	if err := os.WriteFile(path, []byte(original), 0o644); err != nil {
		t.Fatal(err)
	}

	m := withStore(t, newTestWorkspace(t))
	m = m.WithFS(vfs.Disk{})
	m = loadFile(m, path, original)
	docID := m.view.DocID()
	if docID == 0 {
		t.Skip("store not available — docID is 0")
	}

	// Externally change the file after load — the victim's CAS expectation
	// (saved_obs, from Load) no longer matches disk.
	if err := os.WriteFile(path, []byte(external), 0o644); err != nil {
		t.Fatal(err)
	}

	m.opentabs = m.opentabs.MarkDirtyByID(docID)
	m.pendingDataLoss = pendingDataLoss{
		kind:   actionEvict,
		victim: opentabs.TabHandle{DocID: docID, Path: path},
	}

	_, cmd := m.evictSave()
	if cmd == nil {
		t.Fatal("evictSave returned nil cmd — store required for this test")
	}
	msg := cmd()
	saveErr, ok := msg.(FileSaveErrorMsg)
	if !ok || !saveErr.Conflict {
		t.Fatalf("§1.4.7: evictSave must refuse to write a diverged file; got %#v", msg)
	}

	diskBytes, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("ReadFile victim: %v", err)
	}
	if string(diskBytes) != external {
		t.Fatalf("§1.4.7 violation: evictSave clobbered an externally-changed file;\n  disk=%q\n  want=%q",
			string(diskBytes), external)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// D-responses: conflict guard (GuardMerge) response routing
// ─────────────────────────────────────────────────────────────────────────────

// TestConflictGuard_SaveAnywayClears: DataLossSaveAnyway with an active
// pendingConflict clears the conflict and launches a force-write via CAS
// against the captured conflicting observation (freshObs).
func TestConflictGuard_SaveAnywayClears(t *testing.T) {
	m, _, _ := setupSaveConflict(t, "ours", "theirs")

	m2, cmd := m.Update(footer.DataLossGuardResponseMsg{Response: footer.DataLossSaveAnyway})

	if m2.pendingConflict.active {
		t.Fatal("DataLossSaveAnyway: pendingConflict still active after response")
	}
	if cmd == nil {
		t.Fatal("DataLossSaveAnyway: expected a materialize cmd, got nil")
	}
	if !m2.activeSave.InFlight {
		t.Fatal("DataLossSaveAnyway: activeSave.InFlight should be true")
	}
}

// TestConflictGuard_SaveAnywayWrites: DataLossSaveAnyway force-writes the LIVE
// editor buffer to disk — the CAS check accepts the write because disk still
// matches the conflicting bytes captured at detection (freshObs); if it
// changed AGAIN, Materialize would raise a fresh conflict instead.
func TestConflictGuard_SaveAnywayWrites(t *testing.T) {
	m, path, _ := setupSaveConflict(t, "ours original", "theirs on disk")

	const liveContent = "ours live buffer content"
	m.editor = m.editor.SetContent(liveContent)

	m, saveCmd := m.Update(footer.DataLossGuardResponseMsg{Response: footer.DataLossSaveAnyway})
	if saveCmd == nil {
		t.Fatal("expected save cmd")
	}
	result := saveCmd()
	if _, ok := result.(FileSavedMsg); !ok {
		t.Fatalf("expected FileSavedMsg from force-write, got %T: %v", result, result)
	}

	if b, _ := os.ReadFile(path); string(b) != liveContent {
		t.Fatalf("force-write: disk=%q, want live buffer %q", b, liveContent)
	}
}

// TestConflictGuard_SaveAnywayUsesLiveBuffer: even when the editor buffer is
// modified AFTER the conflict was detected (simulating a dictation edit that
// arrived while the guard prompt was visible), [S]ave-anyway must write the
// post-detection buffer.
func TestConflictGuard_SaveAnywayUsesLiveBuffer(t *testing.T) {
	m, path, _ := setupSaveConflict(t, "initial ours", "theirs")

	const laterEdit = "ours WITH post-detection edits"
	m.editor = m.editor.SetContent(laterEdit)

	m, saveCmd := m.Update(footer.DataLossGuardResponseMsg{Response: footer.DataLossSaveAnyway})
	if saveCmd == nil {
		t.Fatal("expected save cmd")
	}
	result := saveCmd()
	if _, ok := result.(FileSavedMsg); !ok {
		t.Fatalf("expected FileSavedMsg, got %T: %v", result, result)
	}
	if b, _ := os.ReadFile(path); string(b) != laterEdit {
		t.Fatalf("SaveAnyway wrote stale capture instead of live buffer:\n  disk=%q\n  want=%q", b, laterEdit)
	}
}

// TestConflictGuard_MergeUsesLiveBuffer: [M]erge must run the 3-way merge
// against the LIVE editor buffer at the moment of the key press, re-probed
// fresh from disk (Fix A) — never a stale detection-time capture.
func TestConflictGuard_MergeUsesLiveBuffer(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "note.md")
	const ancestor = "shared line\nbase line\n"
	if err := os.WriteFile(path, []byte(ancestor), 0o644); err != nil {
		t.Fatal(err)
	}

	m := withStore(t, newTestWorkspace(t))
	m = m.WithFS(vfs.Disk{})
	m = loadFile(m, path, ancestor)
	docID := m.view.DocID()
	if docID == 0 {
		t.Skip("store not available")
	}

	// theirs (disk) == ancestor ⇒ the 3-way merge resolves entirely to ours.
	const liveEdit = "shared line\nLIVE post-detection edit\n"
	m.editor = m.editor.SetContent(liveEdit)

	m.pendingConflict = pendingConflict{active: true, path: path, docID: docID}
	m = runMergeAction(t, m, footer.DataLossMerge)

	if got := m.editor.Content(); got != liveEdit {
		t.Fatalf("Merge used a stale capture instead of the live buffer:\n  buffer=%q\n  want=%q", got, liveEdit)
	}
}

// TestConflictGuard_DiscardLoadsTheirs: DataLossDiscard with an active
// pendingConflict re-probes disk fresh (Fix A) and replaces the editor buffer
// with theirs.
func TestConflictGuard_DiscardLoadsTheirs(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "note.md")
	const oursOriginal = "ours original content"
	if err := os.WriteFile(path, []byte(oursOriginal), 0o644); err != nil {
		t.Fatal(err)
	}

	m := withStore(t, newTestWorkspace(t))
	m = m.WithFS(vfs.Disk{})
	m = loadFile(m, path, oursOriginal)
	docID := m.view.DocID()
	if docID == 0 {
		t.Skip("store not available")
	}

	const theirsContent = "external theirs content"
	if err := os.WriteFile(path, []byte(theirsContent), 0o644); err != nil {
		t.Fatal(err)
	}

	m.pendingConflict = pendingConflict{active: true, path: path, docID: docID}
	m = runMergeAction(t, m, footer.DataLossDiscard)

	if m.pendingConflict.active {
		t.Fatal("DataLossDiscard: pendingConflict still active")
	}
	if got := m.editor.Content(); got != theirsContent {
		t.Fatalf("DataLossDiscard: editor=%q, want %q", got, theirsContent)
	}
	if m.diskChangedHint {
		t.Fatal("DataLossDiscard: diskChangedHint should be false after loading theirs")
	}
}

// TestConflictGuard_MergeClearsConflict: DataLossMerge with an active
// pendingConflict clears it and enters merge mode in the editor.
func TestConflictGuard_MergeClearsConflict(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "note.md")
	const ancestorContent = "shared\noriginal\n"
	const oursContent = "shared\nours changed\n"
	const theirsContent = "shared\ntheirs changed\n"
	if err := os.WriteFile(path, []byte(ancestorContent), 0o644); err != nil {
		t.Fatal(err)
	}

	m := withStore(t, newTestWorkspace(t))
	m = m.WithFS(vfs.Disk{})
	m = loadFile(m, path, ancestorContent)
	docID := m.view.DocID()
	if docID == 0 {
		t.Skip("store not available")
	}
	// A REAL journaled edit diverges ours from the ancestor — without it,
	// only theirs would have changed since Load, which resolves cleanly
	// (never entering the resolver) instead of producing a genuine conflict.
	m = diverge(t, m, docID, ancestorContent, oursContent)

	if err := os.WriteFile(path, []byte(theirsContent), 0o644); err != nil {
		t.Fatal(err)
	}

	m.pendingConflict = pendingConflict{active: true, path: path, docID: docID}
	m = runMergeAction(t, m, footer.DataLossMerge)

	if m.pendingConflict.active {
		t.Fatal("DataLossMerge: pendingConflict still active")
	}
	if !mergemode.IsActive(m.merge) {
		t.Fatal("DataLossMerge: merge resolver not active after merge response")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// guardMergeOptions wiring (unaffected by WP5)
// ─────────────────────────────────────────────────────────────────────────────

func TestGuardMergeOptions_EscIsCancel(t *testing.T) {
	if len(guardMergeOptions) == 0 {
		t.Fatal("guardMergeOptions must not be empty")
	}
	last := guardMergeOptions[len(guardMergeOptions)-1]
	if last.Response != footer.DataLossCancel {
		t.Fatalf("guardMergeOptions: last option must be DataLossCancel (Esc-safety / R4); got %v", last.Response)
	}
	if last.Key != 0 {
		t.Fatalf("guardMergeOptions: last option key must be 0 (Esc sentinel); got %q", last.Key)
	}
}

func TestGuardMergeOptions_SaveAnywayPresent(t *testing.T) {
	for _, opt := range guardMergeOptions {
		if opt.Response == footer.DataLossSaveAnyway && opt.Key == 's' {
			return
		}
	}
	t.Fatal("guardMergeOptions: missing 's' → DataLossSaveAnyway option")
}

func TestGuardMergeOptions_MergePresent(t *testing.T) {
	for _, opt := range guardMergeOptions {
		if opt.Response == footer.DataLossMerge && opt.Key == 'm' {
			return
		}
	}
	t.Fatal("guardMergeOptions: missing 'm' → DataLossMerge option")
}

func TestGuardMergeOptions_DiscardPresent(t *testing.T) {
	for _, opt := range guardMergeOptions {
		if opt.Response == footer.DataLossDiscard && opt.Key == 'd' {
			return
		}
	}
	t.Fatal("guardMergeOptions: missing 'd' → DataLossDiscard option")
}

// ─────────────────────────────────────────────────────────────────────────────
// FileSaveErrorMsg{Conflict}: the entry point that raises the conflict guard
// (replaces the pre-v4 async theirsReadMsg round trip — raiseConflictGuard is
// now pure SQLite, synchronous, since Materialize already captured the
// conflicting disk bytes via I1).
// ─────────────────────────────────────────────────────────────────────────────

func TestFileSaveErrorMsg_ConflictRaisesGuardMerge(t *testing.T) {
	m := withStore(t, newTestWorkspace(t))
	m = loadFile(m, "/fake/path.md", "ours content")
	docID := m.view.DocID()
	if docID == 0 {
		t.Skip("store not available")
	}

	freshHash, err := m.store.PutBlob("theirs content\n")
	if err != nil {
		t.Fatal(err)
	}

	m.activeSave = SaveIdentity{RequestID: "r1", InFlight: true, Path: "/fake/path.md", DocID: docID}
	m, _ = m.Update(FileSaveErrorMsg{
		Path: "/fake/path.md", DocID: docID, RequestID: "r1",
		Conflict: true,
		Fresh:    docstate.Observation{BlobHash: freshHash},
	})

	if !m.pendingConflict.active {
		t.Fatal("FileSaveErrorMsg{Conflict}: pendingConflict should be active after a conflicting save")
	}
	if !m.footer.InGuard() {
		t.Fatal("FileSaveErrorMsg{Conflict}: footer should be in guard mode (GuardMerge)")
	}
	if m.footer.GuardKind() != footer.GuardMerge {
		t.Fatalf("FileSaveErrorMsg{Conflict}: guard kind=%v, want GuardMerge", m.footer.GuardKind())
	}
}

func TestFileSaveErrorMsg_MissingRaisesGuardDeleted(t *testing.T) {
	m := withStore(t, newTestWorkspace(t))
	m = loadFile(m, "/fake/path.md", "ours content")
	docID := m.view.DocID()
	if docID == 0 {
		t.Skip("store not available")
	}

	m.activeSave = SaveIdentity{RequestID: "r1", InFlight: true, Path: "/fake/path.md", DocID: docID}
	m, _ = m.Update(FileSaveErrorMsg{
		Path: "/fake/path.md", DocID: docID, RequestID: "r1",
		Missing: true,
	})

	if !m.pendingDeleted.active {
		t.Fatal("FileSaveErrorMsg{Missing}: GuardDeleted should be raised, not GuardMerge")
	}
	if m.footer.GuardKind() != footer.GuardDeleted {
		t.Fatalf("FileSaveErrorMsg{Missing}: guard kind=%v, want GuardDeleted", m.footer.GuardKind())
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// R2 save-gating
// ─────────────────────────────────────────────────────────────────────────────

// TestR2SaveGating_UnresolvedConflictsBlock: ⌘S with unresolved conflicts must
// not write to disk, and (BUG3) must NOT re-raise the external-change GuardMerge.
func TestR2SaveGating_UnresolvedConflictsBlock(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "note.md")
	const ancestorContent = "shared\noriginal\n"
	const oursContent = "shared\nours changed\n"
	if err := os.WriteFile(path, []byte(ancestorContent), 0o644); err != nil {
		t.Fatal(err)
	}

	m := withStore(t, newTestWorkspace(t))
	m = m.WithFS(vfs.Disk{})
	m = loadFile(m, path, ancestorContent)
	docID := m.view.DocID()
	if docID == 0 {
		t.Skip("store not available")
	}
	// A REAL journaled edit diverges ours from the ancestor — a genuine
	// two-way conflict.
	m = diverge(t, m, docID, ancestorContent, oursContent)

	const theirsContent = "shared\ntheirs changed\n"
	if err := os.WriteFile(path, []byte(theirsContent), 0o644); err != nil {
		t.Fatal(err)
	}

	m.pendingConflict = pendingConflict{active: true, path: path, docID: docID}
	m = runMergeAction(t, m, footer.DataLossMerge)

	if !mergemode.IsActive(m.merge) {
		t.Skip("merge resolver not active (no conflict from libgit2)")
	}
	if !mergemode.HasUnresolvedConflicts(m.merge) {
		t.Skip("no unresolved conflicts (clean merge)")
	}

	beforeDisk, _ := os.ReadFile(path)
	m, saveCmd := m.startSave()

	if m.footer.InGuard() && m.footer.GuardKind() == footer.GuardMerge {
		t.Fatal("BUG3: ⌘S while merging must NOT re-raise the external-change GuardMerge")
	}
	if m.activeSave.InFlight {
		t.Fatal("R2: activeSave.InFlight should be false — no write attempted")
	}
	if saveCmd != nil {
		if _, ok := saveCmd().(FileSavedMsg); ok {
			t.Fatal("R2: ⌘S with unresolved conflicts must not produce a FileSavedMsg (no write)")
		}
	}

	afterDisk, _ := os.ReadFile(path)
	if string(afterDisk) != string(beforeDisk) {
		t.Fatal("R2: ⌘S with unresolved conflicts wrote to disk")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// R2: guard-response routing precedence (conflict-Esc vs. dirty-guard)
// ─────────────────────────────────────────────────────────────────────────────

// TestR2_ConflictEscThenDirtyDiscardRoutes: conflict-Esc then dirty-guard
// [D]iscard must route to the dirty discard (close path), NOT to
// handleDataLossDiscardConflict (which would load theirs).
func TestR2_ConflictEscThenDirtyDiscardRoutes(t *testing.T) {
	m := withStore(t, newTestWorkspace(t))
	m = loadFile(m, "/fake/path.md", "ours content")

	m.pendingConflict = pendingConflict{active: true, path: "/fake/path.md", docID: m.view.DocID()}
	m.footer = m.footer.SetGuard(footer.GuardMerge, guardMergeOptions)

	m, _ = m.Update(footer.DataLossGuardResponseMsg{Response: footer.DataLossCancel})
	if m.pendingConflict.active {
		t.Fatal("R2: Esc must clear pendingConflict")
	}

	m.pendingDataLoss = pendingDataLoss{kind: actionClose}
	m.footer = m.footer.SetGuard(footer.GuardDirty, dataLossGuardOptions)

	m, _ = m.Update(footer.DataLossGuardResponseMsg{Response: footer.DataLossDiscard})

	if m.editor.Content() == "theirs content" {
		t.Fatal("R2: [D]iscard after Esc routed to conflict discard (loaded theirs) instead of dirty discard")
	}
	if m.pendingDataLoss.kind != actionNone {
		t.Fatalf("R2: pendingDataLoss.kind=%v after discard, want actionNone", m.pendingDataLoss.kind)
	}
}
