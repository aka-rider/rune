package workspace

import (
	"os"
	"path/filepath"
	"testing"

	"rune/pkg/ui/components/footer"
	"rune/pkg/vfs"
)

// TestConflictDuringCloseSave_CoexistsThenAbandonsClose is A3/A4's critic-R1
// migration test: "a conflict guard raised during a close/evict/quit save
// survives to its ack" is checked here in the sense the plan's design notes
// actually verify against live code (workspace_edit.go's diverged branch,
// workspace_conflict.go's handleDataLoss* bodies) — the COEXISTENCE window
// is legal and exercised (guard.kind reads guardConflict while
// guard.close.active is still true underneath, proving the kind-first
// dispatcher does not treat that combination as illegal), but resolving the
// conflict guard ABANDONS the close continuation rather than resuming it:
// handleDataLossSaveAnyway/handleDataLossDiscardConflict/handleDataLossMerge
// all unconditionally call abandonDirtyContinuation (see their bodies) —
// there is no re-arm anywhere in the codebase, and isCloseSaveAck can never
// correlate a conflict-guard resolution's own save (different RequestID) to
// the original close. This test pins that ACTUAL, preserved behavior
// precisely so a future change to either direction (illegal-coexistence
// panic, OR a silent switch to resume-semantics) is caught.
func TestConflictDuringCloseSave_CoexistsThenAbandonsClose(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "note.md")
	const ancestorContent = "shared\noriginal\n"
	const oursContent = "shared\nours changed locally\n"
	const theirsContent = "shared\ntheirs changed externally\n"
	if err := os.WriteFile(path, []byte(ancestorContent), 0o644); err != nil {
		t.Fatal(err)
	}

	m := withStore(t, newTestWorkspace(t))
	m = m.WithFS(vfs.Disk{})
	m = loadFile(m, path, ancestorContent)
	docID := m.view.DocID()
	if docID == 0 {
		t.Fatal("setup: store not available")
	}

	// A REAL journaled edit — the doc is genuinely dirty, exactly like a
	// real keystroke session (mirrors enterRealConflict's own recipe).
	m = diverge(t, m, docID, ancestorContent, oursContent)

	// ^W: the doc is dirty, so this raises the ordinary dirty-close guard —
	// NOT a conflict yet (nothing on disk has changed at this point).
	m, cmd := m.requestCloseCurrent()
	if cmd != nil {
		t.Fatalf("requestCloseCurrent: unexpected Cmd on guard-raise: %v", cmd)
	}
	if !m.footer.InGuard() || m.guard.kind != guardDirtyClose {
		t.Fatalf("setup: expected guardDirtyClose prompting, got InGuard=%v kind=%v", m.footer.InGuard(), m.guard.kind)
	}
	if !m.guard.close.active {
		t.Fatal("setup: expected guard.close.active")
	}

	// The external writer lands FOR REAL, after the guard is already up —
	// mirrors abortQuitForDivergence's own "the file may have moved again"
	// framing: the user's [S]ave response is about to discover this.
	if err := os.WriteFile(path, []byte(theirsContent), 0o644); err != nil {
		t.Fatal(err)
	}
	// A real session would have the file watcher's dirChangedMsg -> probeDocCmd
	// -> handleProbeResult round trip record this sighting BEFORE the [S]ave
	// keypress lands (Sync is a pure comparison of ALREADY-recorded facts —
	// docstate/probe.go — it does not itself re-read disk). Probe directly to
	// pre-record the sighting, exactly like that watcher round trip would,
	// WITHOUT going through raiseDeletedGuard/setDiskChangedHint (this write
	// is not a deletion and must not raise the passive disk-changed hint —
	// only the [S]ave attempt below is under test).
	if _, err := m.store.Probe(docID); err != nil {
		t.Fatalf("setup: Probe: %v", err)
	}

	// [S]ave in response to the dirty-close guard: handleKeyPress's
	// guard-owns-keyboard gate already cleared guard.phase synchronously
	// (mirrored here by going straight to the async response, matching
	// runMergeAction's own idiom) — confirmGuardSave moves guard.phase to
	// guardAwaitingSave, then startSave's vetSave discovers SyncDiverged and
	// calls raiseConflictGuard INSTEAD of writing, WITHOUT clearing
	// guard.close (workspace_edit.go's diverged branch returns before the
	// requestID-stamping code below it runs).
	m, cmd = m.Update(footer.DataLossGuardResponseMsg{Response: footer.DataLossSave})
	m = settle(t, m, cmd)

	// Coexistence (critic R1): kind flipped to guardConflict — a DIFFERENT
	// guard now owns the prompt — while the close intent independently
	// survives underneath. Neither field was illegally clobbered by the
	// other; both readings are simultaneously true.
	if m.guard.kind != guardConflict {
		t.Fatalf("expected guard.kind=guardConflict after the close-save hit a divergence, got %v", m.guard.kind)
	}
	if !m.footer.InGuard() || m.footer.GuardKind() != footer.GuardMerge {
		t.Fatalf("expected GuardMerge prompting, got InGuard=%v kind=%v", m.footer.InGuard(), m.footer.GuardKind())
	}
	if !m.guard.conflict.active {
		t.Fatal("expected guard.conflict.active after raiseConflictGuard")
	}
	if !m.guard.close.active {
		t.Fatal("R1: the close intent must survive a conflict guard raised on top of it — guard.close was cleared")
	}
	if m.guard.close.requestID != "" {
		t.Fatal("R1: the close intent's requestID must stay unstamped — the diverged branch returns before startSave's requestID-stamping code runs")
	}
	// The tab must NOT have closed — the guard, not the close, currently owns the view.
	if m.view.DocID() != docID {
		t.Fatalf("close must not have executed while the conflict guard is up: view.DocID()=%d, want %d", m.view.DocID(), docID)
	}

	// Resolve the conflict guard with [D]iscard (load theirs) — one of the
	// three conflict resolutions (Save-anyway/Discard/Merge all share this
	// unconditional-clear behavior; Discard is the cheapest to drive here).
	m, cmd = m.Update(footer.DataLossGuardResponseMsg{Response: footer.DataLossDiscard})
	m = settle(t, m, cmd)

	// Documented, preserved behavior (NOT a resume): resolving the conflict
	// guard abandons the close continuation rather than completing it — the
	// user must press ^W again if they still want to close. Both guards are
	// now fully idle; nothing lingers.
	if m.guard.kind != guardNone || m.footer.InGuard() {
		t.Fatalf("expected both guards idle after the conflict resolved, got guard.kind=%v InGuard=%v", m.guard.kind, m.footer.InGuard())
	}
	if m.guard.close.active {
		t.Fatal("expected the abandoned close intent to have cleared, got guard.close.active=true")
	}
	if m.view.DocID() != docID {
		t.Fatalf("close must not have executed after the conflict resolved (abandon semantics, not resume): view.DocID()=%d, want %d", m.view.DocID(), docID)
	}
	if got := m.editor.Content(); got != theirsContent {
		t.Fatalf("DataLossDiscard should have loaded theirs: editor.Content()=%q, want %q", got, theirsContent)
	}
}
