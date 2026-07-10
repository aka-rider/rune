package workspace

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	tea "charm.land/bubbletea/v2"

	dictengine "rune/pkg/dictation"
	"rune/pkg/ui/components/footer"
	"rune/pkg/ui/pages/workspace/mergemode"
	"rune/pkg/vfs"
)

// ─────────────────────────────────────────────────────────────────────────────
// WP2 regression tests — each REQUIRED to fail on the pre-WP2 tree (Data-
// Integrity Model v4 plan, Part V). Failing-run evidence is recorded in the
// WP2 commit / final report, per root CLAUDE.md's failing-test-first rule.
// ─────────────────────────────────────────────────────────────────────────────

// switchToPath drives a real tab-switch through requestOpenPath (the same
// entry point opentabs.TabSelectedMsg and the keyboard tab-switch use) and
// settles the resulting load with settleOneHop — NOT the fully-recursive
// drainCmd: a full drainCmd would also recurse into whatever a landed
// dirChangedMsg/fileChangedMsg re-arms (startWatch's real fsnotify watcher,
// workspace_watch.go), which blocks forever with no timeout of its own
// (mirrors execFastCmds' own doc comment on why it exists) — and, more
// immediately, into disableDictationForTransition's footer.ShowStatusMsg
// auto-dismiss round trip. Stopping at one hop sidesteps both regardless of
// which one is armed in a given test's setup; none of this helper's callers
// need to observe state past that single hop anyway.
func switchToPath(t *testing.T, m Model, docID int64, path string) Model {
	t.Helper()
	m, cmd := m.requestOpenPath(docID, path)
	return settleOneHop(t, m, cmd)
}

// (a)/H2 — a stale resolveProbeMsg (the fresh disk probe launched by
// [D]iscard) landing after the user has switched to a different document
// must never clobber the now-displayed buffer. Pre-WP2, the equivalent
// handler dispatched to applyDiscardConflict/applyMergeConflict with no
// recheck of msg.docID against the CURRENT m.view.DocID() (contrast
// handleUnwindProbe, which already rechecks) — this test reproduces exactly
// that gap.
func TestH2_StaleMergeReadDroppedAfterTabSwitch(t *testing.T) {
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

	// A conflict was detected on A; the user presses [D]iscard. The footer
	// guard clears immediately (production behavior — see
	// handleDataLossGuardResponse's DataLossCancel comment), but the fresh
	// disk-read Cmd this launches has not landed yet — this is the race
	// window.
	m.pendingConflict = pendingConflict{active: true, path: pathA, docID: docA}
	var discardCmd tea.Cmd
	m, discardCmd = m.handleDataLossDiscardConflict()
	if discardCmd == nil {
		t.Fatal("setup: expected a resolveProbeCmd from discard")
	}

	// While that read is in flight, the user switches to a different file B.
	m = switchToPath(t, m, 0, pathB)
	docB := m.view.DocID()
	if m.view.Path() != pathB {
		t.Fatalf("setup: expected view switched to B, got %q", m.view.Path())
	}

	// NOW the stale discard read for A lands.
	for _, msg := range execCmds(discardCmd) {
		m, _ = m.Update(msg)
	}

	if m.view.DocID() != docB || m.view.Path() != pathB {
		t.Fatalf("H2 (must fail pre-WP2): stale mergeReadMsg for doc A changed the active view: docID=%d path=%q, want doc B %q",
			m.view.DocID(), m.view.Path(), pathB)
	}
	if got := m.editor.Content(); got != "B content\n" {
		t.Fatalf("H2 (must fail pre-WP2): stale discard-read for doc A clobbered doc B's buffer: got %q, want %q",
			got, "B content\n")
	}
}

// (b)/H1 — a dictation chunk landing after a tab switch must not reach the
// newly-displayed buffer: the session must be disabled by the switch itself
// (a stale startOff/appliedLen anchor targets whatever buffer is now
// displayed, at byte offsets that belong to a different document).
func TestH1_DictationChunkAfterTabSwitch(t *testing.T) {
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
	m = m.setFocus(paneCenter)
	m.dict = m.dict.Enable(0, m.view.DocID(), m.epoch) // anchored at offset 0 of A's buffer

	m = switchToPath(t, m, 0, pathB)
	if m.view.Path() != pathB {
		t.Fatalf("setup: expected view switched to B, got %q", m.view.Path())
	}

	if m.dict.Enabled() {
		t.Fatal("H1 (must fail pre-WP2): tab switch must disable a stale dictation session")
	}

	// Defense in depth: even if a chunk from the stale session still arrives,
	// it must never mutate the new buffer.
	m, _ = m.Update(dictengine.PartialTranscriptionMsg{Accumulated: "spoken words"})
	if got := m.editor.Content(); got != "B content\n" {
		t.Fatalf("H1 (must fail pre-WP2): stale dictation chunk corrupted the new buffer after tab switch: got %q, want %q",
			got, "B content\n")
	}
}

// (c)/S2 — an AppendEdit failure (store closed underneath the journal write)
// must leave the buffer byte-identical to before the keystroke: journalEdit
// must invert the just-applied edit back into the buffer rather than leaving
// it permanently ahead of the (failed) journal write, and must surface the
// error.
func TestS2_JournalFailureRollsBackBuffer(t *testing.T) {
	m := withStore(t, newTestWorkspace(t))
	m = m.WithFS(vfs.Disk{})
	m = loadFile(m, "/fake/path.md", "hello")
	m = focusEditor(m)
	before := m.editor.Content()

	// Force AppendEdit to fail by closing the store's DB connection underneath it.
	if err := m.store.Close(); err != nil {
		t.Fatalf("close store: %v", err)
	}

	m, cmd := m.Update(tea.KeyPressMsg{Code: 'x', Text: "x"})

	if got := m.editor.Content(); got != before {
		t.Fatalf("S2 (must fail pre-WP2): buffer diverged from journal after a failed AppendEdit: got %q, want %q (rolled back)",
			got, before)
	}
	if cmd == nil {
		t.Fatal("expected an error Cmd surfacing the journal failure")
	}
	foundError := false
	for _, msg := range execCmds(cmd) {
		if e, ok := msg.(footer.ShowErrorMsg); ok && strings.Contains(e.Text, "journal write failed") {
			foundError = true
		}
	}
	if !foundError {
		t.Fatal("S2 (must fail pre-WP2): expected a footer.ShowErrorMsg surfacing the journal write failure")
	}
}

// (d)/S7 — [D]iscard's ReplaceAll must be journaled synchronously (mirrors
// applyMergeConflict), so store.Content(docID) — the crash-recovery
// reconstruction — reflects theirs. A real prior edit (with its own "local"
// autosave snapshot) exists in the journal first: RecoverDocument's
// nearest-snapshot anchor is picked by seq (then id) — without the discard's
// own ReplaceAll journaled as a later event, recovery's replay never advances
// past that pre-discard anchor, silently reconstructing pre-discard content
// (exactly the corruption S7 describes: "recovery reconstructs pre-discard
// content over what the user sees").
func TestS7_DiscardConflictJournalsReplaceAll(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "note.md")
	if err := os.WriteFile(path, []byte("ours\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	m := withStore(t, newTestWorkspace(t))
	m = m.WithFS(vfs.Disk{})
	m = loadFile(m, path, "ours\n")
	docID := m.view.DocID()
	m = focusEditor(m)

	// A real prior edit + its own "local" autosave snapshot already exist in
	// the journal (seq=1) before the conflict is even detected.
	m, _ = m.Update(tea.KeyPressMsg{Code: 'X', Text: "X"})
	preDiscardContent := m.editor.Content()
	seq, err := m.store.CurrentSeq(docID)
	if err != nil {
		t.Fatalf("CurrentSeq: %v", err)
	}
	if _, err := m.store.CreateSnapshot(docID, preDiscardContent, seq); err != nil {
		t.Fatalf("CreateSnapshot: %v", err)
	}

	// External change lands on disk; the user [D]iscards it via the footer
	// guard, which accepts s/d/m/Esc regardless of pane focus — here focus is
	// NOT on the editor (e.g. the guard was resolved from the file tree), so
	// the broadcast path's incidental "focus==paneCenter" drain (workspace_
	// update.go) cannot rescue an undrained ReplaceAll — isolating the real
	// S7 gap in applyDiscardConflict itself.
	m = m.setFocus(paneTree)
	if err := os.WriteFile(path, []byte("theirs on disk\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	m.pendingConflict = pendingConflict{active: true, path: path, docID: docID}
	m = runMergeAction(t, m, footer.DataLossDiscard)

	if got := m.editor.Content(); got != "theirs on disk\n" {
		t.Fatalf("setup: expected editor to show theirs, got %q", got)
	}

	got, err := m.store.Content(docID)
	if err != nil {
		t.Fatalf("store.Content: %v", err)
	}
	if got != "theirs on disk\n" {
		t.Fatalf("S7 (must fail pre-WP2): store.Content(docID) = %q, want %q (the [D]iscard ReplaceAll was never journaled, so recovery reconstructs pre-discard content)",
			got, "theirs on disk\n")
	}
}

// TestDiscardConflictMultibyteCursorAtByteOffset is the workspace-level
// regression test for the §1.5 rune-count-as-byte-offset bug (WP1): [D]iscard
// drives markdownedit.ReplaceAll -> textedit.ReplaceRange, which used to place
// the cursor at start+utf8.RuneCountInString(text) instead of start+len(text).
// TestS7_DiscardConflictJournalsReplaceAll above is ASCII-only and would not
// have caught this — theirs here contains CJK and an emoji (multibyte runes).
func TestDiscardConflictMultibyteCursorAtByteOffset(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "note.md")
	if err := os.WriteFile(path, []byte("ours\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	m := withStore(t, newTestWorkspace(t))
	m = m.WithFS(vfs.Disk{})
	m = loadFile(m, path, "ours\n")
	docID := m.view.DocID()
	m = focusEditor(m)

	theirs := "世界你好 🎉 multibyte\n"
	if err := os.WriteFile(path, []byte(theirs), 0o644); err != nil {
		t.Fatal(err)
	}
	m.pendingConflict = pendingConflict{active: true, path: path, docID: docID}
	m = runMergeAction(t, m, footer.DataLossDiscard)

	if got := m.editor.Content(); got != theirs {
		t.Fatalf("setup: expected editor to show theirs, got %q", got)
	}

	wantOffset := len(theirs) // BYTES (§1.5) — not utf8.RuneCountInString(theirs)
	if got := m.editor.CursorOffset(); got != wantOffset {
		t.Errorf("cursor offset after multibyte [D]iscard: got %d, want %d (byte length of %q)",
			got, wantOffset, theirs)
	}
}

// (e)/H1 — undo must disable an active dictation session anchored on the
// pre-undo buffer: the journal jump can shift or invalidate whatever byte
// range the session's startOff/appliedLen anchor still points at.
func TestH1_DictationDisabledAcrossUndo(t *testing.T) {
	m := withStore(t, newTestWorkspace(t))
	m = m.WithFS(vfs.Disk{})
	m = loadFile(m, "/fake/path.md", "hello")
	m = focusEditor(m)

	m, _ = m.Update(tea.KeyPressMsg{Code: 'x', Text: "x"})
	if got := m.editor.Content(); got != "xhello" {
		t.Fatalf("setup: editor content = %q, want %q", got, "xhello")
	}

	// Arm dictation anchored on the current (post-keystroke) buffer.
	m.dict = m.dict.Enable(m.editor.CursorOffset(), m.view.DocID(), m.epoch)

	m, _ = m.handleUndo()
	if got := m.editor.Content(); got != "hello" {
		t.Fatalf("setup: undo did not revert: got %q", got)
	}

	if m.dict.Enabled() {
		t.Fatal("H1 (must fail pre-WP2): undo must disable a dictation session anchored on the pre-undo buffer")
	}
}

// TestMergeAbort_JournalsRestoreEvenWithoutFurtherEditorFocus locks in the S7
// audit finding for mergemode.Abort's restore path (plan WP2 item 5's "audit
// … for the same gap"): the ONLY call site (workspace_update_keys.go's Esc
// branch inside `case paneCenter:`) already unconditionally drains+journals
// right after Abort's ReplaceAll, in the SAME case block — so, unlike
// applyDiscardConflict pre-fix, this path never needed a WP2 code change.
// This test passes on both the pre- and post-WP2 tree; it exists to make that
// audit conclusion falsifiable rather than a bare claim.
//
// Asserts directly against the journal (AllEdits) rather than
// store.Content/RecoverDocument: RecoverDocument's snapshot-anchor selection
// (ORDER BY seq DESC, id DESC over ALL sources) can pick a same-seq
// source='disk' merge-ancestor snapshot instead of the true load-time
// anchor — a separate, pre-existing structural issue (the "two facts, two
// forced through one mechanism" collision Part III's observations/ancestorAt
// redesign and the WP5 cutover remove `source` to fix) that is out of WP2's
// scope and would make this assertion flaky for reasons unrelated to what
// it's checking.
func TestMergeAbort_JournalsRestoreEvenWithoutFurtherEditorFocus(t *testing.T) {
	m, _, docID := enterRealConflict(t)
	m = m.setFocus(paneCenter)

	m, _ = m.Update(tea.KeyPressMsg{Code: tea.KeyEscape})
	if mergemode.IsActive(m.merge) {
		t.Fatal("setup: Esc must abort and deactivate the merge")
	}
	wantContent := m.editor.Content()

	// Move focus away immediately — no further editor keystroke to
	// incidentally drain anything.
	m = m.setFocus(paneTree)

	batches, err := m.store.AllEdits(docID)
	if err != nil {
		t.Fatalf("AllEdits: %v", err)
	}
	if len(batches) < 2 {
		t.Fatalf("mergemode.Abort restore not journaled: got %d main events, want >= 2 (enter + abort)", len(batches))
	}
	last := batches[len(batches)-1]
	if len(last) != 1 || last[0].Insert != wantContent {
		t.Fatalf("mergemode.Abort restore not journaled correctly: last event = %+v, want a single edit inserting %q", last, wantContent)
	}
}
