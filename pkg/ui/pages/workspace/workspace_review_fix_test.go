package workspace

import (
	"os"
	"path/filepath"
	"testing"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/docstate"
	"rune/pkg/ui/components/footer"
	"rune/pkg/vfs"
)

// Review finding 1: a DiskAhead reopen reached while the shared editor is
// read-only (coming from the Help doc) must still adopt FOR REAL — pre-fix,
// textedit silently dropped the ReplaceAll (read-only guard) while
// resolveAdoptAt advanced saved_obs to theirs anyway: the editor displayed
// the stale reconstruction, the CAS baseline matched disk, and the next ⌘S
// silently overwrote the newer external content.
func TestR1Adopt_FromHelpReadOnlyEditor(t *testing.T) {
	dir := t.TempDir()
	pathA := filepath.Join(dir, "a.md")
	const originalA = "original A content\n"
	const externalA = "external A change while help was open\n"

	m := withStore(t, newTestWorkspace(t))
	m = m.WithFS(vfs.Disk{})
	m = loadFile(m, pathA, originalA)
	docA := m.view.DocID()
	if docA == 0 {
		t.Fatal("store not available")
	}

	// Open Help via the REAL F1 key: view switches to the virtual doc and the
	// editor goes read-only. (The stale-adopt hazard needs exactly this
	// state.) A direct m.toggleHelp() call would bypass handleKeyPress's
	// closing finalize() — the active tab would still point at a.md while the
	// view shows help, a mid-update state settle's invariant sweep
	// (EDITOR-TAB-COH) rightly rejects.
	mm, hc := m.Update(tea.KeyPressMsg{Code: tea.KeyF1})
	m = settle(t, mm, hc)

	// A changes externally while Help is displayed.
	if err := os.WriteFile(pathA, []byte(externalA), 0o644); err != nil {
		t.Fatal(err)
	}

	// Re-open A from Help (filetree selection has no help gate).
	m2, oc := m.requestOpenPath(docA, pathA)
	m = settle(t, m2, oc)

	if got := m.editor.Content(); got != externalA {
		t.Fatalf("editor.Content() = %q, want adopted external content %q (ReplaceAll dropped by read-only editor?)", got, externalA)
	}
	vfsContent, err := m.store.Content(docA)
	if err != nil {
		t.Fatalf("store.Content: %v", err)
	}
	if vfsContent != m.editor.Content() {
		t.Fatalf("store.Content(docA) = %q != editor.Content() = %q — adoption must be journaled, not just baseline-moved", vfsContent, m.editor.Content())
	}
	sync, err := m.store.Sync(docA)
	if err != nil {
		t.Fatalf("Sync: %v", err)
	}
	if sync.Kind != docstate.SyncClean {
		t.Fatalf("Sync after read-only-path adopt = %v, want SyncClean", sync.Kind)
	}
}

// Review finding 9: a close-save that returns Raced raises the raced guard
// and must CANCEL the pending close intent — pre-fix guard.close{active:true}
// stayed dangling, so a later unrelated save ack executed the close out from
// under the user minutes after they chose [K]eep-mine.
func TestRacedCloseSave_ClearsPendingClose(t *testing.T) {
	dir := t.TempDir()
	pathA := filepath.Join(dir, "a.md")

	m := withStore(t, newTestWorkspace(t))
	m = m.WithFS(vfs.Disk{})
	m = loadFile(m, pathA, "content\n")
	docA := m.view.DocID()
	if docA == 0 {
		t.Fatal("store not available")
	}

	// requestID stamped on guard.close mirrors what startSave() does in
	// production when it launches a save as the confirmed continuation of a
	// close guard (workspace_edit.go) — the correlation isCloseSaveAck
	// checks (GUARD-STATE-COH) before letting an ack clear a pending guard.
	m.guard.close = closeIntent{active: true, requestID: "close-save-1"}
	m.activeSave = SaveIdentity{RequestID: "close-save-1", InFlight: true, Path: pathA, DocID: docA}

	msg := FileSavedMsg{
		Path: pathA, DocID: docA, RequestID: "close-save-1",
		Result: docstate.MatResult{Committed: true, Raced: true},
	}
	m2, _ := m.Update(msg)
	m = m2

	if m.guard.close.active {
		t.Fatal("guard.close must be cancelled by a Raced save ack — a later unrelated ack would close the tab without a prompt")
	}
	if !m.guard.raced.active {
		t.Fatal("raced guard must be armed for the displayed doc")
	}
}

// Review finding 8: quit-batch save acks that arrive AFTER the quit was
// aborted (a Raced/diverged sibling cleared pendingDataLoss) must still be
// processed — pre-fix they fell through every branch, so a SECOND race in
// the same batch vanished with no guard and no notice.
func TestOrphanedQuitAck_RacedStillSurfaces(t *testing.T) {
	dir := t.TempDir()
	pathA := filepath.Join(dir, "a.md")
	pathB := filepath.Join(dir, "b.md")

	m := withStore(t, newTestWorkspace(t))
	m = m.WithFS(vfs.Disk{})
	m = loadFile(m, pathA, "a content\n")
	docA := m.view.DocID()
	m = loadFile(m, pathB, "b content\n") // B displayed; A in background
	if docA == 0 || m.view.DocID() == 0 {
		t.Fatal("store not available")
	}

	// Quit already aborted: guard.quit is zero. A's quit-batch ack arrives
	// late, carrying a race.
	msg := FileSavedMsg{
		Path: pathA, DocID: docA, RequestID: "quitsave-1-0-99999",
		Result: docstate.MatResult{Committed: true, Raced: true},
	}
	m2, _ := m.Update(msg)
	m = m2

	if _, queued := m.racedQueue[docA]; !queued {
		t.Fatal("an orphaned quit-batch Raced ack must queue the raced guard for the background doc — pre-fix it fell through every handler branch and the race vanished silently")
	}
}

// GUARD-STATE-COH regression (rune-fuzz-investigator seed 0a72fa2e0fa043d9:
// 'j', KeyUp, Shift+Left, Ctrl+W): committing an untitled doc's title (which
// starts an async bind-new save via title.Commit()'s deferred RenameRequestMsg
// — workspace_update.go's RenameRequestMsg case) and, in the SAME keypress,
// Ctrl+W synchronously calling requestCloseCurrent BEFORE that save's ack
// lands, races an unrelated pendingDataLoss{actionClose}+GuardDirty against
// the bind-new save's own activeSave/RequestID. Neither handler stamps
// pendingDataLoss.requestID with the bind-new save's ID (only startSave does,
// and only for the confirmed close-save continuation), so isCloseSaveAck must
// report false and the close guard must survive untouched — whichever way the
// bind-new save resolves.

// TestBindNewRace_ErrorAck_PreservesUnrelatedCloseGuard covers the ack shape
// still reachable after the Fix-1 Materialize path fix (real disk errors:
// permission denied, disk full, a RenameExcl no-clobber conflict on the
// autogenerated name).
func TestBindNewRace_ErrorAck_PreservesUnrelatedCloseGuard(t *testing.T) {
	m := withStore(t, newTestWorkspace(t))
	docID := m.view.DocID()
	if docID == 0 || !m.view.IsUntitled() {
		t.Fatal("store not available / no startup untitled")
	}
	m = focusEditor(m)
	m, _ = m.Update(tea.KeyPressMsg{Code: 'j', Text: "j"})
	if m.editor.Content() == "" {
		t.Fatal("setup: editor content empty after typing")
	}

	// The bind-new save is already in flight (its RenameRequestMsg handler
	// ran); THEN — before its ack — an unrelated Ctrl+W raised its own close
	// guard on the SAME dirty doc, with no requestID correlating it to this
	// save. Set guard.kind/phase directly (mirrors raiseGuardPrompt's own
	// effect) so the kind-first dispatcher reads this as a real dirty-close
	// guard, exactly like requestCloseCurrent would have left it.
	const bindReqID = "bind-12345"
	newPath := filepath.Join(t.TempDir(), "Untitled 1.md")
	m.activeSave = SaveIdentity{RequestID: bindReqID, InFlight: true, Path: newPath, DocID: docID}
	m.guard.close = closeIntent{active: true}
	m = m.raiseGuardPrompt(guardDirtyClose)

	msg := FileSaveErrorMsg{Path: newPath, DocID: docID, RequestID: bindReqID, Err: errTest}
	m, _ = m.Update(msg)

	if !m.guard.close.active {
		t.Fatal("GUARD-STATE-COH: an unrelated bind-new save's error ack cleared the close guard's guard.close")
	}
	if !m.footer.InGuard() || m.footer.GuardKind() != footer.GuardDirty {
		t.Fatalf("GUARD-STATE-COH: close guard no longer showing after the unrelated ack: InGuard=%v kind=%v", m.footer.InGuard(), m.footer.GuardKind())
	}

	// A subsequent Discard must resolve the close, never quit the app — the
	// kind-first dispatcher routes DataLossDiscard via guard.kind==
	// guardDirtyClose specifically (A4), so there is no "unrecognized intent
	// falls through to quit" default catch-all left to regress into.
	// executeClose's CreateUntitled resets the editor to blank (§ its own
	// SetContent("")) — checking the typed content is gone is robust even
	// when the replacement untitled doc happens to reuse the same rowid
	// (SQLite's plain INTEGER PRIMARY KEY reuses max(rowid)+1 once the table
	// is emptied by DeleteDoc, so comparing DocIDs isn't a reliable signal).
	m, _ = m.Update(footer.DataLossGuardResponseMsg{Response: footer.DataLossDiscard})
	if m.guard.close.active {
		t.Fatal("Discard did not resolve guard.close")
	}
	if m.editor.Content() == "j" {
		t.Fatal("Discard did not close/discard the dirty buffer (editor still shows the typed content) — did it quit instead?")
	}
}

// TestBindNewRace_SuccessAck_PreservesUnrelatedCloseGuard covers the ack shape
// that Fix 1 makes the COMMON case: the bind-new save now actually succeeds.
// Before Fix 1 this branch was unreachable for a genuine untitled doc (the
// save always errored first), so the race could only ever surface via the
// error path — this success path was the gap an adversarial review caught.
func TestBindNewRace_SuccessAck_PreservesUnrelatedCloseGuard(t *testing.T) {
	m := withStore(t, newTestWorkspace(t))
	docID := m.view.DocID()
	if docID == 0 || !m.view.IsUntitled() {
		t.Fatal("store not available / no startup untitled")
	}
	m = focusEditor(m)
	m, _ = m.Update(tea.KeyPressMsg{Code: 'j', Text: "j"})
	if m.editor.Content() == "" {
		t.Fatal("setup: editor content empty after typing")
	}

	const bindReqID = "bind-67890"
	newPath := filepath.Join(t.TempDir(), "Untitled 1.md")
	m.activeSave = SaveIdentity{RequestID: bindReqID, InFlight: true, Path: newPath, DocID: docID}
	m.guard.close = closeIntent{active: true}
	m = m.raiseGuardPrompt(guardDirtyClose)

	msg := FileSavedMsg{
		Path: newPath, DocID: docID, RequestID: bindReqID, BindNew: true,
		Result: docstate.MatResult{Committed: true},
	}
	m, _ = m.Update(msg)

	// The unrelated close guard must be untouched: the bind-new save has no
	// idea a close was requested and must not silently execute someone else's
	// still-pending decision.
	if !m.guard.close.active {
		t.Fatal("GUARD-STATE-COH: an unrelated bind-new save's success ack cleared the close guard's guard.close")
	}
	if !m.footer.InGuard() || m.footer.GuardKind() != footer.GuardDirty {
		t.Fatalf("GUARD-STATE-COH: close guard no longer showing after the unrelated ack: InGuard=%v kind=%v", m.footer.InGuard(), m.footer.GuardKind())
	}
	if !tabHasDocID(m, docID) {
		t.Fatal("the bind-new save's success silently closed the tab it has no idea a close was pending for")
	}
}
