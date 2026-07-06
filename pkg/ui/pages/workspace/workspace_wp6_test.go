package workspace

// WP6 regression / delayed-message tests (Data-Integrity Model v4 plan, Part
// V, WP6). Failing-run evidence for the "must fail pre-WP6" test is recorded
// in the WP6 commit / final report, per root CLAUDE.md's failing-test-first
// rule.

import (
	"os"
	"path/filepath"
	"testing"

	tea "charm.land/bubbletea/v2"

	dictengine "rune/pkg/dictation"
	"rune/pkg/editor/buffer"
	"rune/pkg/ui/components/footer"
	"rune/pkg/vfs"
)

// TestH3_StoreDirtyBackgroundTabIncludedInQuitSave is the WP6 must-fail-
// pre-fix regression for H3: saveAllDirtyForQuit (via dirtyHandles) and the
// ConfirmQuitMsg guard (via anyDirty) must consult the STORE's ground truth
// (docstate.DirtyDocs/IsDirty), never opentabs' cached per-tab flag alone —
// a background tab whose cache drifted clean while the store still holds an
// unsaved edit must still be included/guarded at quit (§1.4.8).
func TestH3_StoreDirtyBackgroundTabIncludedInQuitSave(t *testing.T) {
	dir := t.TempDir()
	pathA := filepath.Join(dir, "a.md")
	pathB := filepath.Join(dir, "b.md")
	if err := os.WriteFile(pathA, []byte("A content\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(pathB, []byte("B content\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	m := withStore(t, newTestWorkspace(t))
	m = m.WithFS(vfs.Disk{})
	m = loadFile(m, pathA, "A content\n")
	docA := m.view.DocID()
	if docA == 0 {
		t.Skip("store not available")
	}

	// Switch to B — A becomes a background tab, clean (nothing edited yet).
	m = loadFile(m, pathB, "B content\n")
	if m.opentabs.HasDirty() {
		t.Fatal("setup: expected clean before manipulation")
	}

	// Manipulate A's store state DIRECTLY (a real journaled edit, bypassing
	// the normal journalEdit→MarkDirtyByID path a keystroke would take) —
	// exactly the H3 scenario: the store now holds an unsaved edit for A,
	// but opentabs' cached flag for A was never told.
	if _, err := m.store.AppendEdit(docA, []buffer.AppliedEdit{{Insert: "X"}}, nil, nil); err != nil {
		t.Fatalf("AppendEdit: %v", err)
	}

	// The cached flag must indeed still read clean — confirms this reproduces
	// the H3 gap (a genuinely dirty store, a stale-clean cache), not
	// something else.
	if m.opentabs.HasDirty() {
		t.Fatal("setup invariant broken: opentabs cache already reflects dirty — test no longer isolates H3")
	}

	// ^C^C quit-save's dirty set must still include A.
	handles := m.dirtyHandles()
	found := false
	for _, h := range handles {
		if h.DocID == docA {
			found = true
		}
	}
	if !found {
		t.Fatal("H3 (must fail pre-WP6): quit-save's dirty set excluded a store-dirty background tab whose cache drifted clean")
	}

	// The quit guard itself must raise, never silently quit past it.
	if !m.anyDirty() {
		t.Fatal("H3 (must fail pre-WP6): anyDirty() must detect the store-dirty background tab")
	}
	m, _ = m.Update(footer.ConfirmQuitMsg{})
	if !m.footer.InGuard() {
		t.Fatal("H3 (must fail pre-WP6): ConfirmQuitMsg must raise the dirty guard, never quit past a store-dirty background tab")
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// Delayed-message tests (Part IV viewTicket chokepoint)
// ─────────────────────────────────────────────────────────────────────────────

// TestDelayed_ResolveProbeAcrossTabSwitch_RefusedWithNotice is the WP6
// re-verification of H2 under the ticket mechanism (applyViewResult): a
// resolveProbeMsg (the fresh-probe result [D]/[M] launches) landing after
// the user has switched to a different document must be refused (a footer
// notice, never a silent no-op) rather than applied to the now-displayed
// buffer.
func TestDelayed_ResolveProbeAcrossTabSwitch_RefusedWithNotice(t *testing.T) {
	dir := t.TempDir()
	pathA := filepath.Join(dir, "a.md")
	pathB := filepath.Join(dir, "b.md")
	if err := os.WriteFile(pathA, []byte("A ours\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(pathB, []byte("B content\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	m := withStore(t, newTestWorkspace(t))
	m = m.WithFS(vfs.Disk{})
	m = loadFile(m, pathA, "A ours\n")
	docA := m.view.DocID()
	if docA == 0 {
		t.Skip("store not available")
	}

	m.pendingConflict = pendingConflict{active: true, path: pathA, docID: docA}
	var discardCmd tea.Cmd
	m, discardCmd = m.handleDataLossDiscardConflict()
	if discardCmd == nil {
		t.Fatal("setup: expected a resolveProbeCmd from discard")
	}

	// While that probe is in flight, the user switches to B — bumps epoch.
	m = switchToPath(t, m, 0, pathB)
	if m.view.Path() != pathB {
		t.Fatalf("setup: expected view switched to B, got %q", m.view.Path())
	}
	beforeContent := m.editor.Content()

	// NOW the stale resolveProbeMsg for A lands.
	for _, msg := range execCmds(discardCmd) {
		m, _ = m.Update(msg)
	}

	if m.view.Path() != pathB {
		t.Fatalf("stale resolveProbeMsg for doc A changed the active view: got %q, want B %q", m.view.Path(), pathB)
	}
	if got := m.editor.Content(); got != beforeContent {
		t.Fatalf("stale resolveProbeMsg mutated B's buffer: got %q, want unchanged %q", got, beforeContent)
	}
}

// TestDelayed_DictationChunkAcrossUndo_RefusedAndAutoDisabled: a dictation
// chunk landing after an undo replaced the buffer (bumpEpoch, Part IV) must
// be refused and the session auto-disabled — the structural ticket backstop,
// not just the scattered Disable() call at the undo site.
func TestDelayed_DictationChunkAcrossUndo_RefusedAndAutoDisabled(t *testing.T) {
	m := withStore(t, newTestWorkspace(t))
	m = m.WithFS(vfs.Disk{})
	m = loadFile(m, "/fake/path.md", "hello")
	m = focusEditor(m)

	m, _ = m.Update(tea.KeyPressMsg{Code: 'x', Text: "x"})
	if got := m.editor.Content(); got != "xhello" {
		t.Fatalf("setup: editor content = %q, want %q", got, "xhello")
	}

	// Arm dictation anchored on the current (post-keystroke) buffer/epoch.
	m.dict = m.dict.Enable(m.editor.CursorOffset(), m.view.DocID(), m.epoch)
	if !m.dict.Enabled() {
		t.Fatal("setup: expected dictation enabled")
	}
	preUndoEpoch := m.epoch

	m, _ = m.handleUndo()
	if got := m.editor.Content(); got != "hello" {
		t.Fatalf("setup: undo did not revert: got %q", got)
	}
	if m.epoch == preUndoEpoch {
		t.Fatal("setup: undo must bump epoch (Part IV)")
	}

	// disableDictationForTransition already disabled it immediately — the
	// structural ticket check is a backstop for whenever that's NOT hit; both
	// must agree the session is gone.
	if m.dict.Enabled() {
		t.Fatal("undo must disable a dictation session anchored on the pre-undo epoch")
	}

	// Defense in depth: even if a stale chunk somehow still arrived, it must
	// never mutate the post-undo buffer.
	beforeContent := m.editor.Content()
	m, _ = m.Update(dictengine.PartialTranscriptionMsg{Accumulated: "spoken words"})
	if got := m.editor.Content(); got != beforeContent {
		t.Fatalf("stale dictation chunk after undo mutated the buffer: got %q, want unchanged %q", got, beforeContent)
	}
}

// TestDictationTicket_ChatFocusedControl is a control confirming the new
// ticket check (routeDictationEdit) does not regress ordinary chat-focused
// dictation: chatDocID is stable across the session, so a chunk anchored
// there must still apply normally with no intervening transition.
func TestDictationTicket_ChatFocusedControl(t *testing.T) {
	m := withStore(t, newTestWorkspace(t))
	m = m.WithFS(vfs.Disk{})
	m.focus = paneChat

	m.dict = m.dict.Enable(0, m.chatDocID, m.epoch)
	if !m.dict.Enabled() {
		t.Fatal("setup: expected dictation enabled")
	}

	m, _ = m.Update(dictengine.PartialTranscriptionMsg{Accumulated: "spoken words"})

	if got := m.chat.PromptContent(); got != "spoken words" {
		t.Fatalf("control: chat dictation edit not applied: got %q, want %q", got, "spoken words")
	}
	if !m.dict.Enabled() {
		t.Fatal("control: session must remain enabled — nothing invalidated its ticket")
	}
}

// TestApplyViewResult_SaveAckCommitsAfterTabSwitch is the WP6 INVERSION test
// (Part IV I3): a save ack (FileSavedMsg) is a DOC-targeted result, not a
// view-targeted one — it must commit (mark the tab clean) even after the
// user has switched away and the ticket that would gate a view-targeted
// result is long stale. The store-side commit already happened inside
// Materialize regardless (S5); this pins that the UI-side ack handling never
// silently drops the tab-clean transition just because the view moved on.
func TestApplyViewResult_SaveAckCommitsAfterTabSwitch(t *testing.T) {
	dir := t.TempDir()
	pathA := filepath.Join(dir, "a.md")
	pathB := filepath.Join(dir, "b.md")
	if err := os.WriteFile(pathA, []byte("A content\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(pathB, []byte("B content\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	m := withStore(t, newTestWorkspace(t))
	m = m.WithFS(vfs.Disk{})
	m = loadFile(m, pathA, "A content\n")
	docA := m.view.DocID()
	if docA == 0 {
		t.Skip("store not available")
	}
	m = focusEditor(m)
	m, _ = m.Update(tea.KeyPressMsg{Code: 'X', Text: "X"})

	m, saveCmd := m.startSave()
	if saveCmd == nil {
		t.Fatal("setup: expected a materialize cmd")
	}

	// Switch away BEFORE the save ack lands — bumps epoch, changes m.view.
	m = switchToPath(t, m, 0, pathB)
	if m.view.Path() != pathB {
		t.Fatalf("setup: expected view switched to B, got %q", m.view.Path())
	}

	// NOW the save ack for A lands.
	msg := saveCmd()
	savedMsg, ok := msg.(FileSavedMsg)
	if !ok {
		t.Fatalf("expected FileSavedMsg, got %T: %v", msg, msg)
	}
	m, _ = m.Update(savedMsg)

	// The store's own commit already happened inside Materialize (S5) —
	// verify docstate agrees A is no longer dirty, proving the ack was never
	// gated on a view ticket.
	dirty, err := m.store.IsDirty(docA)
	if err != nil {
		t.Fatalf("IsDirty: %v", err)
	}
	if dirty {
		t.Fatal("inversion: save ack for A must commit (mark clean) even though the view moved to B")
	}
	// B must remain completely unaffected.
	if m.view.Path() != pathB {
		t.Fatalf("save ack for A corrupted the displayed doc: view=%q, want B %q", m.view.Path(), pathB)
	}
}
