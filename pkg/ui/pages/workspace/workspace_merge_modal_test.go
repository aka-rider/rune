package workspace

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	tea "charm.land/bubbletea/v2"

	dictengine "rune/pkg/dictation"
	"rune/pkg/editor/buffer"
	"rune/pkg/editor/cursor"
	"rune/pkg/ui/components/footer"
	"rune/pkg/ui/pages/workspace/mergemode"
	"rune/pkg/vfs"
)

// diverge journals a REAL edit from oldContent to newContent (mirrors a
// genuine keystroke session's AppendEdit) and updates the live editor buffer
// to match. Used to make ours genuinely diverge from the ancestor Load just
// recorded — WITHOUT it, a subsequent 3-way merge against a different theirs
// only has ONE side changed (theirs), which auto-resolves cleanly instead of
// producing the genuine two-way conflict these tests need.
func diverge(t *testing.T, m Model, docID int64, oldContent, newContent string) Model {
	t.Helper()
	// Deleted must be set — buffer.ReplayForward skips len(Deleted) bytes at
	// Start (not End-Start), so omitting it replays as a pure insert with
	// oldContent's tail left concatenated on.
	if _, err := m.store.AppendEdit(docID,
		[]buffer.AppliedEdit{{Start: 0, End: len(oldContent), Deleted: oldContent, Insert: newContent}}, nil, nil); err != nil {
		t.Fatalf("diverge: AppendEdit: %v", err)
	}
	m.editor, _ = m.editor.SetContent(newContent)
	return m
}

// settleOneHop executes cmd's own leaves (a Fix A/B fresh-disk-read Cmd is a
// fast, single fsys.ReadFile) and feeds each resulting message back into
// m.Update ONCE, discarding whatever second-level Cmd that produces. Unlike
// settle, this deliberately does NOT recurse into a full settle(): some
// callers assert on the state exactly one hop past the async read/probe
// landing — e.g. TestUndoPastEnter_ReRaisesConflictOnSave and
// TestUndoPastEnter_DiskEqualsTheirs_NeverSilentOverwrite assert on
// m.footer.InGuard()/GuardKind() and m.activeSave.InFlight right after that
// single hop, before any further round trip (such as the autosave flush a
// successful journal write schedules) has a chance to run. Callers with no
// such intermediate-state dependency should use settle instead — this
// helper exists only for the ones that genuinely need to inspect that
// one-hop-settled state.
func settleOneHop(t *testing.T, m Model, cmd tea.Cmd) Model {
	t.Helper()
	for _, msg := range execCmds(cmd) {
		m, _ = m.Update(msg)
	}
	return m
}

// runMergeAction drives a DataLossGuardResponseMsg for the conflict guard
// through Fix A's two-phase fresh-disk-read (press → resolveProbeCmd →
// resolveProbeMsg → apply) — exactly the async round trip the real Bubble
// Tea runtime performs — via settle. None of runMergeAction's callers
// assert on state before a successful resolution's autosave flush lands, so
// (unlike settleOneHop's remaining callers) there is no reason to stop short
// of a full settle here.
func runMergeAction(t *testing.T, m Model, response footer.DataLossGuardResponse) Model {
	t.Helper()
	m, cmd := m.Update(footer.DataLossGuardResponseMsg{Response: response})
	return settle(t, m, cmd)
}

// enterRealConflict wires up a workspace with a real, file-backed conflict and
// fires [M]erge, exactly mirroring TestR2SaveGating_UnresolvedConflictsBlock /
// TestConflictGuard_MergeClearsConflict (a proven-reliable recipe for a genuine
// libgit2 conflict in this environment — not a t.Skip-guarded one). Returns
// the workspace with mergemode active and one unresolved conflict, plus the
// disk path and docID.
func enterRealConflict(t *testing.T) (Model, string, int64) {
	t.Helper()
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

	// A REAL journaled edit diverges ours from the ancestor Load just
	// recorded — a genuine TWO-way conflict (ours AND theirs both diverged),
	// not the clean auto-resolve a single-sided change would produce.
	m = diverge(t, m, docID, ancestorContent, oursContent)

	// Simulate the external edit landing FOR REAL on disk: Fix A's [M] reads
	// FRESH disk at the action (theirs is re-probed live, never a stale
	// detection-time capture).
	if err := os.WriteFile(path, []byte(theirsContent), 0o644); err != nil {
		t.Fatal(err)
	}

	m.guard.prompt = promptPayload{path: path, docID: docID}
	m = m.raiseGuardPrompt(guardConflict) // A3: keep guard.kind/phase coherent with the hand-set intent (kind-first dispatch reads guard.kind now)
	m = runMergeAction(t, m, footer.DataLossMerge)
	if !mergemode.IsActive(m.merge) {
		t.Fatal("enterRealConflict: expected merge active after [M]")
	}
	if !mergemode.HasUnresolvedConflicts(m.merge) {
		t.Fatal("enterRealConflict: expected an unresolved conflict")
	}
	return m, path, docID
}

// ─────────────────────────────────────────────────────────────────────────────
// Guard → diff immediately: [M]erge renders the real ours-vs-theirs diff
// ─────────────────────────────────────────────────────────────────────────────

func TestMergeGuard_RendersDiffImmediately(t *testing.T) {
	m, _, _ := enterRealConflict(t)

	view := m.merge.View()
	if !strings.Contains(view, "ours changed") {
		t.Errorf("merge view missing ours content: %q", view)
	}
	if !strings.Contains(view, "theirs changed") {
		t.Errorf("merge view missing theirs content: %q", view)
	}

	// The center pane must actually substitute the merge view while active.
	body := m.View().Content
	if !strings.Contains(body, "⚙ Merge") {
		t.Errorf("workspace View() must show the persistent merge footer hint while active:\n%s", body)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// Modal transition refusal (§4) — no path backgrounds/closes/renames/quits a
// mid-merge doc; each shows a footer hint and no-ops.
// ─────────────────────────────────────────────────────────────────────────────

func TestModalMerge_TabSwitchRefused(t *testing.T) {
	m, path, _ := enterRealConflict(t)

	otherPath := filepath.Join(filepath.Dir(path), "other.md")
	if err := os.WriteFile(otherPath, []byte("other\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	var cmd tea.Cmd
	m, cmd = m.requestOpenPath(0, otherPath)

	if !mergemode.IsActive(m.merge) {
		t.Fatal("tab-switch must be refused while merge is unresolved — merge deactivated")
	}
	if m.view.Path() != path {
		t.Fatalf("tab-switch must be refused: view switched to %q, want still %q", m.view.Path(), path)
	}
	if cmd == nil {
		t.Fatal("expected a footer-hint Cmd from the refusal")
	}
}

func TestModalMerge_CloseRefused(t *testing.T) {
	m, path, _ := enterRealConflict(t)

	m, _ = m.requestCloseCurrent()

	if !mergemode.IsActive(m.merge) {
		t.Fatal("close must be refused while merge is unresolved")
	}
	if m.view.Path() != path {
		t.Fatalf("close must be refused: view changed to %q, want still %q", m.view.Path(), path)
	}
}

func TestModalMerge_QuitRefused(t *testing.T) {
	m, _, _ := enterRealConflict(t)

	m2, cmd := m.Update(footer.ConfirmQuitMsg{})

	if !mergemode.IsActive(m2.merge) {
		t.Fatal("quit must be refused while merge is unresolved")
	}
	if m2.footer.InGuard() {
		t.Fatal("quit-refusal must not raise the dirty guard — it must simply no-op with a hint")
	}
	if cmd == nil {
		t.Fatal("expected a footer-hint Cmd from the refusal")
	}
	// Must not have produced a teardown-and-quit sequence (tea.Quit).
	for _, msg := range execCmds(cmd) {
		if _, ok := msg.(tea.QuitMsg); ok {
			t.Fatal("quit must be refused while merge is unresolved — got tea.QuitMsg")
		}
	}
}

func TestModalMerge_NewFileRefused(t *testing.T) {
	m, path, _ := enterRealConflict(t)

	// ⌘N (ctrl+n) must be refused while unresolved: CreateUntitled does
	// SetContent("") over the hidden marker buffer, backgrounding the mid-merge
	// doc so its markers could later reach disk on quit-save (rung-1). Drive the
	// real key path (global key case, not a direct helper call).
	m = m.setFocus(paneCenter)
	m2, cmd := m.Update(tea.KeyPressMsg{Code: 'n', Mod: tea.ModCtrl})

	if !mergemode.IsActive(m2.merge) {
		t.Fatal("⌘N must be refused while merge is unresolved — merge deactivated")
	}
	if m2.view.IsUntitled() || m2.view.Path() != path {
		t.Fatalf("⌘N must not background the merge doc: view=%q untitled=%v, want still %q", m2.view.Path(), m2.view.IsUntitled(), path)
	}
	if m2.focus == paneTitle {
		t.Fatal("⌘N refusal must not fall through to setFocus(paneTitle)")
	}
	if got := m2.editor.Content(); !strings.Contains(got, "<<<<<<< ours") {
		t.Fatalf("⌘N refusal must leave the marker working buffer intact, got %q", got)
	}
	if cmd == nil {
		t.Fatal("expected a footer-hint Cmd from the refusal")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// Esc aborts the modal merge → reverts to ours, transitions allowed again
// ─────────────────────────────────────────────────────────────────────────────

func TestModalMerge_EscAborts_RevertsAndUnblocksTransitions(t *testing.T) {
	m, path, _ := enterRealConflict(t)
	preMergeOurs := m.merge // capture before abort only to read State-independent facts if needed

	m = m.setFocus(paneCenter)
	m, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyEscape})

	if mergemode.IsActive(m.merge) {
		t.Fatal("Esc must abort and deactivate the merge")
	}
	if got := m.editor.Content(); got != "shared\nours changed\n" {
		t.Fatalf("Esc-abort: buffer=%q, want exact pre-merge ours %q", got, "shared\nours changed\n")
	}
	_ = preMergeOurs

	// Transitions must be allowed again now that merge is inactive.
	otherPath := filepath.Join(filepath.Dir(path), "other.md")
	if err := os.WriteFile(otherPath, []byte("other\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	m, _ = m.requestOpenPath(0, otherPath)
	if mergemode.IsActive(m.merge) {
		t.Fatal("post-abort transition must proceed normally")
	}
}

// TestModalMerge_UnrelatedGuardCancelMustNotClearActiveMerge: Fix D's preview
// clear-on-Cancel is scoped to !mergemode.IsActive — the DataLossCancel
// response is shared by EVERY guard kind's Esc (dirty/merge/deleted/trash),
// not just GuardMerge. Neither FocusExplorer nor FileDeleteRequestedMsg is
// gated on HasUnresolvedConflicts, so the user CAN switch to the file tree and
// raise+cancel an UNRELATED GuardTrash prompt while a real merge is active on
// the main doc. That Cancel must never wipe the active resolver's bookkeeping
// out from under the still-marker-laden buffer (which would desync
// HasUnresolvedConflicts() from the buffer's real content — a save-gating
// hole, since R2 reads the resolver as done while raw markers remain).
func TestModalMerge_UnrelatedGuardCancelMustNotClearActiveMerge(t *testing.T) {
	m, _, _ := enterRealConflict(t)
	if !mergemode.IsActive(m.merge) {
		t.Fatal("setup: expected merge active")
	}
	bufferBefore := m.editor.Content()

	// An unrelated guard (e.g. GuardTrash from the file tree) is raised and
	// then cancelled while mid-merge.
	m.footer = m.footer.SetGuard(footer.GuardTrash, trashGuardOptions)
	m, _ = m.Update(footer.DataLossGuardResponseMsg{Response: footer.DataLossCancel})

	if !mergemode.IsActive(m.merge) {
		t.Fatal("unrelated guard Cancel wrongly cleared the ACTIVE merge bookkeeping")
	}
	if !mergemode.HasUnresolvedConflicts(m.merge) {
		t.Fatal("unrelated guard Cancel wrongly cleared unresolved conflicts")
	}
	if got := m.editor.Content(); got != bufferBefore {
		t.Fatalf("buffer changed unexpectedly: got %q, want %q", got, bufferBefore)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// Footer hint sync
// ─────────────────────────────────────────────────────────────────────────────

func TestSyncMergeHint_TracksActiveAndConflictsLeft(t *testing.T) {
	m, _, _ := enterRealConflict(t)
	m = m.syncMergeHint()
	view := m.footer.View()
	if !strings.Contains(view, "Merge") {
		t.Errorf("footer must show the merge hint while active: %q", view)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// Undo-past-Enter: resolve → ⌘Z past the marker-load → merge exits → ⌘S must
// re-raise the conflict, never silently overwrite theirs (§4/§1.4.7).
// ─────────────────────────────────────────────────────────────────────────────

func TestUndoPastEnter_ReRaisesConflictOnSave(t *testing.T) {
	m, path, _ := enterRealConflict(t)

	// Resolve the (only) conflict with [O] through the REAL key path (paneCenter
	// pre-intercept), so the accept is journaled exactly as production does.
	m = m.setFocus(paneCenter)
	m, _ = m.Update(tea.KeyPressMsg{Code: 'o', Text: "o"})
	if mergemode.IsActive(m.merge) {
		t.Fatal("expected merge to auto-exit after resolving the only conflict")
	}

	// ⌘Z #1: undo the [O] accept — reopens the block (merge active again).
	var undoCmd tea.Cmd
	m, undoCmd = m.Update(tea.KeyPressMsg{Code: 'z', Mod: tea.ModCtrl})
	m = settleOneHop(t, m, undoCmd)
	if !mergemode.IsActive(m.merge) {
		t.Fatal("undo #1 must reopen the resolved block (Resync re-derives active state)")
	}

	// ⌘Z #2: undo the Enter marker-load — buffer returns to pre-merge ours;
	// Resync finds no blocks and exits merge. Fix B's re-detection read
	// (mergeUnwindReadCmd) fires HERE (active→inactive transition) — settle it
	// (a real, fast fsys.ReadFile) so the guard re-raise/baseline-restamp
	// actually lands before the ⌘S assertion below.
	m, undoCmd = m.Update(tea.KeyPressMsg{Code: 'z', Mod: tea.ModCtrl})
	m = settleOneHop(t, m, undoCmd)
	if mergemode.IsActive(m.merge) {
		t.Fatal("undo #2 (past Enter) must exit merge mode")
	}
	if got := m.editor.Content(); got != "shared\nours changed\n" {
		t.Fatalf("after undo-past-Enter: buffer=%q, want the original pre-merge content", got)
	}

	// Fix B's re-detection (settled above, after undo #2) must have found the
	// restored pre-merge-ours buffer diverges from the FRESH theirs it just
	// read and re-raised GuardMerge — disk holds theirsContent for real
	// (enterRealConflict writes it before [M]), so a naive ⌘S here would
	// otherwise silently overwrite it with pre-merge ours (rung-1).
	if !m.footer.InGuard() || m.footer.GuardKind() != footer.GuardMerge {
		t.Fatalf("undo-past-Enter: expected GuardMerge re-raised by Fix B's re-detection; InGuard=%v kind=%v",
			m.footer.InGuard(), m.footer.GuardKind())
	}

	// Drive a REAL ⌘S through the full key-routing path (handleKeyPress's
	// Priority 2.1 InGuard gate must intercept it BEFORE startSave/
	// materializeCmd ever run — this is the actual production safety
	// mechanism Fix B relies on, not a buffer-vs-disk write-boundary guard).
	beforeDisk, _ := os.ReadFile(path)
	var saveKeyCmd tea.Cmd
	m, saveKeyCmd = m.Update(tea.KeyPressMsg{Code: 's', Mod: tea.ModSuper})
	m = settleOneHop(t, m, saveKeyCmd)

	if m.activeSave.InFlight {
		t.Fatal("undo-past-Enter ⌘S: must NOT start a save — the guard must intercept the key first")
	}
	afterDisk, _ := os.ReadFile(path)
	if string(afterDisk) != string(beforeDisk) {
		t.Fatalf("undo-past-Enter ⌘S must NOT silently overwrite: disk changed from %q to %q", beforeDisk, afterDisk)
	}
}

// TestUndoPastEnter_DiskEqualsTheirs_NeverSilentOverwrite is the MANDATORY
// regression test for BUG3's rung-1 hole: TestUndoPastEnter_ReRaisesConflictOnSave
// passed even before Fix B only because its ORIGINAL fixture left disk==ours
// (the R3 backstop's content-compare then legitimately disagreed and refused
// the write for an unrelated reason). The actual silent-overwrite fires when
// disk==theirs: the durable ancestor snapshot [M] stamps IS theirs, so an
// invalidated-but-otherwise-untested baseline lets the R3 backstop see
// disk-matches-ancestor and wave the write through — straight over theirs,
// with the user's pre-merge ours (rung-1, §0). Self-contained (does not use
// enterRealConflict) so this scenario is unambiguous regardless of future
// fixture changes.
func TestUndoPastEnter_DiskEqualsTheirs_NeverSilentOverwrite(t *testing.T) {
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
		t.Fatal("store not available")
	}
	// A REAL journaled edit diverges ours from the ancestor — a genuine
	// two-way conflict.
	m = diverge(t, m, docID, ancestorContent, oursContent)

	// Disk genuinely holds THEIRS — the external editor's real write, laid
	// down AFTER loadFile's own disk write (loadFile writes its content
	// argument to disk as part of the real store.Load round trip) so the
	// fresh probe [M] performs actually observes theirsContent, not
	// whatever loadFile itself last wrote.
	if err := os.WriteFile(path, []byte(theirsContent), 0o644); err != nil {
		t.Fatal(err)
	}

	m.guard.prompt = promptPayload{path: path, docID: docID}
	m = m.raiseGuardPrompt(guardConflict) // A3: keep guard.kind/phase coherent with the hand-set intent (kind-first dispatch reads guard.kind now)
	m = runMergeAction(t, m, footer.DataLossMerge)
	if !mergemode.IsActive(m.merge) || !mergemode.HasUnresolvedConflicts(m.merge) {
		t.Fatal("setup: expected merge active with one unresolved conflict after [M]")
	}

	// Resolve with [O] through the real key path, then undo twice (past Enter).
	m = m.setFocus(paneCenter)
	m, _ = m.Update(tea.KeyPressMsg{Code: 'o', Text: "o"})
	if mergemode.IsActive(m.merge) {
		t.Fatal("setup: expected merge to auto-exit after resolving the only conflict")
	}

	var undoCmd tea.Cmd
	m, undoCmd = m.Update(tea.KeyPressMsg{Code: 'z', Mod: tea.ModCtrl})
	m = settleOneHop(t, m, undoCmd)
	m, undoCmd = m.Update(tea.KeyPressMsg{Code: 'z', Mod: tea.ModCtrl})
	m = settleOneHop(t, m, undoCmd)

	if mergemode.IsActive(m.merge) {
		t.Fatal("setup: undo-past-Enter must exit merge mode")
	}
	if got := m.editor.Content(); got != oursContent {
		t.Fatalf("setup: after undo-past-Enter buffer=%q, want %q", got, oursContent)
	}

	// MANDATORY assertion 1: GuardMerge must be RE-RAISED (Fix B re-detection),
	// not silently cleared.
	if !m.footer.InGuard() || m.footer.GuardKind() != footer.GuardMerge {
		t.Fatalf("BUG3 (disk==theirs): expected GuardMerge re-raised after undo-past-Enter; InGuard=%v kind=%v",
			m.footer.InGuard(), m.footer.GuardKind())
	}

	// MANDATORY assertion 2: a subsequent ⌘S, driven through the REAL key
	// path, must NEVER write pre-merge ours over theirs.
	beforeDisk, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if string(beforeDisk) != theirsContent {
		t.Fatalf("setup invariant broken: disk=%q, want theirsContent=%q before the ⌘S attempt", beforeDisk, theirsContent)
	}
	var saveKeyCmd tea.Cmd
	m, saveKeyCmd = m.Update(tea.KeyPressMsg{Code: 's', Mod: tea.ModSuper})
	m = settleOneHop(t, m, saveKeyCmd)

	if m.activeSave.InFlight {
		t.Fatal("BUG3 (disk==theirs): ⌘S must not start a save — the guard must intercept it first")
	}
	afterDisk, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if string(afterDisk) != theirsContent {
		t.Fatalf("BUG3 (disk==theirs): SILENT OVERWRITE — disk changed from theirsContent to %q (pre-merge ours=%q)",
			afterDisk, oursContent)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// E1 — dictation must never mutate the hidden marker buffer mid-merge
// ─────────────────────────────────────────────────────────────────────────────

// TestModalMerge_DictationDrainBlockedMidMerge: a dictation session already
// anchored on the editor, with a merge active, must not have its transcribed
// text land in the (hidden) marker buffer — that would desync mergemode's
// block-span tracking and could corrupt which bytes [O]/[T] next collapse a
// block to (data safety). The session must stop rather than silently drop
// every future chunk with no feedback.
func TestModalMerge_DictationDrainBlockedMidMerge(t *testing.T) {
	m, _, _ := enterRealConflict(t)
	m = m.setFocus(paneCenter)
	preContent := m.editor.Content()

	// Arm a dictation session anchored at the top of the (hidden) marker
	// buffer, exactly as ⌃v/footer.DictationStartMsg would.
	m.dict = m.dict.Enable(0, m.view.DocID(), m.epoch)
	if !m.dict.Enabled() {
		t.Fatal("setup: expected dictation enabled")
	}

	// A partial transcription lands while merge is active.
	m, _ = m.Update(dictengine.PartialTranscriptionMsg{Accumulated: "spoken words"})

	if got := m.editor.Content(); got != preContent {
		t.Fatalf("E1: dictation mutated the hidden marker buffer mid-merge:\n  got=%q\n  want=%q", got, preContent)
	}
	if !mergemode.IsActive(m.merge) {
		t.Fatal("E1: merge must still be active — the dropped dictation edit must not have desynced/aborted it")
	}
	if !mergemode.HasUnresolvedConflicts(m.merge) {
		t.Fatal("E1: the conflict must still be unresolved (untouched by the dropped dictation edit)")
	}
	if m.dict.Enabled() {
		t.Fatal("E1: the dictation session must be stopped, not left listening to silently drop every future chunk")
	}
}

// TestModalMerge_DictationDrainWorksNormallyOutsideMerge is a control: the
// SAME dictation drain, with NO merge active, must still apply normally —
// confirming E1's gate is scoped to mid-merge, not a blanket dictation break.
func TestModalMerge_DictationDrainWorksNormallyOutsideMerge(t *testing.T) {
	m := newTestWorkspace(t)
	m = loadFile(m, "/fake/path.md", "hello ")
	m = m.setFocus(paneCenter)
	m.editor = m.editor.SetCursors([]cursor.Cursor{{Position: len("hello ")}})
	m.dict = m.dict.Enable(m.editor.CursorOffset(), m.view.DocID(), m.epoch)

	m, _ = m.Update(dictengine.PartialTranscriptionMsg{Accumulated: "world"})

	if got := m.editor.Content(); got != "hello world" {
		t.Fatalf("control: dictation edit not applied outside merge: got %q, want %q", got, "hello world")
	}
	if !m.dict.Enabled() {
		t.Fatal("control: dictation session must stay enabled outside merge")
	}
}
