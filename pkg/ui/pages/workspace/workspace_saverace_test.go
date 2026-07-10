package workspace

import (
	"os"
	"path/filepath"
	"testing"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/docstate"
	"rune/pkg/vfs"
)

// drainCmd delivers cmd's message (and any message a resulting Cmd yields,
// recursively) through m.Update, fully settling an async round trip within a
// single deterministic test step — no time.Sleep, no reliance on runtime
// scheduling.
func drainCmd(m Model, cmd tea.Cmd) Model {
	pending := execCmds(cmd)
	for len(pending) > 0 {
		msg := pending[0]
		pending = pending[1:]
		var next tea.Cmd
		m, next = m.Update(msg)
		pending = append(pending, execCmds(next)...)
	}
	return m
}

// clickTabRow computes the (x,y) screen coordinates opentabs' handleMouseClick
// expects for the tab at idx (0-based), mirroring workspace_view.go's
// recalcLayout/paneAtPoint layout math, and self-verifies the point actually
// resolves to paneTabs before returning — so a future layout change fails this
// helper loudly instead of silently clicking the wrong pane.
func clickTabRow(t *testing.T, m Model, idx int) (x, y int) {
	t.Helper()
	contentH := m.totalHeight - m.footer.Height()
	innerH := contentH - 2
	otH := m.opentabs.Height()
	avail := innerH - otH
	ftH := avail
	if ftH < 4 {
		ftH = 4
	}
	if ftH > avail {
		ftH = avail
	}
	x = m.leftPaneW / 2
	y = ftH + 2 + idx // SetOffset(1, ftH+1); opentabs.handleMouseClick: idx = y - offsetY - 1
	if pane, ok := m.paneAtPoint(x, y); !ok || pane != paneTabs {
		t.Fatalf("clickTabRow(%d): (%d,%d) resolves to pane=%v ok=%v, want paneTabs", idx, x, y, pane, ok)
	}
	return x, y
}

// clickTab simulates a real mouse click on the tab at idx and drives the full
// round trip (click → handleMouseClick → opentabs.Update → TabSelectedMsg →
// checkStatOnFocus/requestOpenPath → possibly beginLoad → FileLoadedMsg) to
// completion. This test file exercises the actual mouse-click input path —
// the point of the regression — never a Msg constructed by hand in place of a
// real click.
func clickTab(t *testing.T, m Model, idx int) Model {
	t.Helper()
	x, y := clickTabRow(t, m, idx)
	m, cmd := m.Update(tea.MouseClickMsg{X: x, Y: y, Button: tea.MouseLeft})
	return drainCmd(m, cmd)
}

// saveRaceFixture creates a store-backed workspace over real files on disk
// (vfs.Disk, so every save genuinely churns the inode via atomicfile.Write —
// unlike vfs.Mem, which only assigns a new inode on a path's first write) and
// loads the named files as tabs in order, returning the workspace and each
// file's resolved docID.
func saveRaceFixture(t *testing.T, contents map[string]string, order []string) (Model, map[string]int64) {
	t.Helper()
	dir := t.TempDir()
	paths := make(map[string]string, len(order))
	for _, name := range order {
		p := filepath.Join(dir, name)
		if err := os.WriteFile(p, []byte(contents[name]), 0o644); err != nil {
			t.Fatal(err)
		}
		paths[name] = p
	}

	m := withStore(t, newTestWorkspace(t))
	m = m.WithFS(vfs.Disk{})

	docIDs := make(map[string]int64, len(order))
	for _, name := range order {
		m = loadFile(m, paths[name], contents[name])
		docID := m.view.DocID()
		if docID == 0 {
			t.Fatalf("expected a real docID for %s", name)
		}
		docIDs[name] = docID
	}
	return m, docIDs
}

// startSaveGetAck starts an interactive save on the currently displayed
// document and physically performs the atomic rewrite now (execCmds runs
// materializeStoreCmd's closure synchronously) — deterministic, no
// goroutine/timing dependency. The resulting FileSavedMsg is returned
// unDELIVERED so the test can interleave other input before acking it.
func startSaveGetAck(t *testing.T, m Model) (Model, FileSavedMsg) {
	t.Helper()
	m, cmd := m.startSave()
	if !m.activeSave.InFlight {
		t.Fatal("expected activeSave.InFlight after startSave")
	}
	msgs := execCmds(cmd)
	if len(msgs) != 1 {
		t.Fatalf("expected exactly one message from materializeStoreCmd, got %d: %+v", len(msgs), msgs)
	}
	fsMsg, ok := msgs[0].(FileSavedMsg)
	if !ok {
		t.Fatalf("expected FileSavedMsg, got %T: %+v", msgs[0], msgs[0])
	}
	return m, fsMsg
}

// TestMouseClickRace_ReopenDefersUntilSaveSettles reproduces the mouse-click/
// save race the atomic-rewrite investigation found: handleMouseClick has no
// InFlight gate (unlike handleKeyPress's Priority-2 "save in flight" block),
// so clicking away from a saving tab and back before its FileSavedMsg lands
// could reach handleFileLoadedMsg's ungated store.OpenPath before
// docstate.Bind re-stamps the atomic rewrite's new inode — orphaning the
// doc's undo/snapshot history onto a fresh docID. Reverting requestOpenPath's
// savingTarget gate (workspace_nav.go) makes this test fail: A's reload fires
// immediately instead of deferring, and — because vfs.Disk churns the inode
// on every save — the premature OpenPath forks A onto a new docID.
func TestMouseClickRace_ReopenDefersUntilSaveSettles(t *testing.T) {
	m, docIDs := saveRaceFixture(t, map[string]string{
		"a.md": "a-v1",
		"b.md": "b-v1",
	}, []string{"a.md", "b.md"})
	docIDA, docIDB := docIDs["a.md"], docIDs["b.md"]

	m = clickTab(t, m, 0) // switch back to A (tab order: [a.md, b.md])
	if m.view.DocID() != docIDA {
		t.Fatalf("setup: expected view=A (%d), got %d", docIDA, m.view.DocID())
	}

	m, fsMsg := startSaveGetAck(t, m)

	// Click away to B — a real navigation, not deferred (B is not the saving target).
	m = clickTab(t, m, 1)
	if m.view.DocID() != docIDB {
		t.Fatalf("after clicking B: view=%d, want %d", m.view.DocID(), docIDB)
	}

	// Click back to A BEFORE delivering A's FileSavedMsg. The fix: this must
	// defer, not reload immediately.
	m = clickTab(t, m, 0)
	if m.view.DocID() != docIDB {
		t.Fatalf("clicking back to A mid-save must defer, not reload immediately: view=%d, want B=%d", m.view.DocID(), docIDB)
	}
	if !m.pendingReopen.active || m.pendingReopen.docID != docIDA {
		t.Fatalf("expected a deferred reopen for A (%d), got %+v", docIDA, m.pendingReopen)
	}

	// Settle A's save. handleFileSavedMsg must Bind A's real identity (msg.DocID,
	// not m.view's — currently B) and then flush the deferred reopen.
	m, cmd := m.Update(fsMsg)
	m = drainCmd(m, cmd)

	// No fork: A's path must still resolve to the SAME docID it started with.
	ref, err := m.store.OpenPath(m.opentabs.PathAt(0))
	if err != nil {
		t.Fatalf("OpenPath(A) after settle: %v", err)
	}
	if ref.ID != docIDA {
		t.Fatalf("save+click-race orphaned history: OpenPath(A) = %d, want original docID %d", ref.ID, docIDA)
	}
	if got := m.opentabs.Len(); got != 2 {
		t.Fatalf("expected exactly 2 tabs (no duplicate), got %d", got)
	}
	// The deferred navigation to A must have replayed once the save settled.
	if m.view.DocID() != docIDA {
		t.Fatalf("expected deferred reopen of A to replay after save settled: view=%d, want %d", m.view.DocID(), docIDA)
	}
}

// TestFileSavedMsg_DoesNotCorruptUnrelatedDisplayedDoc is the simpler variant
// of the mouse-click race: a single click away (no click back needed) is
// enough to corrupt an UNRELATED document, because handleFileSavedMsg used to
// key its store/view mutations off m.view's CURRENT identity rather than the
// save's own captured identity (msg.DocID/msg.Path). Reverting the
// stillDisplayed gate in handleFileSavedMsg makes this test fail: A's
// FileSavedMsg stamps its baseline onto B (the doc the user switched to) and
// rebinds B's docstate row to A's path.
func TestFileSavedMsg_DoesNotCorruptUnrelatedDisplayedDoc(t *testing.T) {
	m, docIDs := saveRaceFixture(t, map[string]string{
		"a.md": "a-v1",
		"b.md": "b-v1",
	}, []string{"a.md", "b.md"})
	docIDA, docIDB := docIDs["a.md"], docIDs["b.md"]

	m = clickTab(t, m, 0) // switch back to A
	if m.view.DocID() != docIDA {
		t.Fatalf("setup: expected view=A (%d), got %d", docIDA, m.view.DocID())
	}

	m, fsMsg := startSaveGetAck(t, m)

	// Click away to B — no click back.
	m = clickTab(t, m, 1)
	if m.view.DocID() != docIDB {
		t.Fatalf("after clicking B: view=%d, want %d", m.view.DocID(), docIDB)
	}
	pathBBefore := m.view.Path()

	// Deliver A's save ack while B is displayed.
	m, cmd := m.Update(fsMsg)
	m = drainCmd(m, cmd)

	// B must be completely unaffected: identity unchanged.
	if m.view.DocID() != docIDB || m.view.Path() != pathBBefore {
		t.Fatalf("A's save ack corrupted the displayed doc: view=(%d,%q), want (%d,%q)",
			m.view.DocID(), m.view.Path(), docIDB, pathBBefore)
	}
	refB, err := m.store.OpenPath(pathBBefore)
	if err != nil {
		t.Fatalf("OpenPath(B): %v", err)
	}
	if refB.ID != docIDB {
		t.Fatalf("A's save hijacked B's docstate identity: OpenPath(B) = %d, want %d", refB.ID, docIDB)
	}
}

// TestRequestOpenPath_DeferredReopenSupersededByNewNavigation: a fresh
// navigation request must supersede a stale deferred reopen, mirroring
// supersedeLoad's "most recent request wins" semantics — otherwise settling
// the original save would yank the user back to a document they've since
// navigated away from a second time.
func TestRequestOpenPath_DeferredReopenSupersededByNewNavigation(t *testing.T) {
	m, docIDs := saveRaceFixture(t, map[string]string{
		"a.md": "a-v1",
		"c.md": "c-v1",
	}, []string{"a.md", "c.md"})
	docIDA, docIDC := docIDs["a.md"], docIDs["c.md"]

	m = clickTab(t, m, 0) // switch back to A
	if m.view.DocID() != docIDA {
		t.Fatalf("setup: expected view=A (%d), got %d", docIDA, m.view.DocID())
	}

	m, fsMsg := startSaveGetAck(t, m)

	m = clickTab(t, m, 1) // switch to C (tab order: [a.md, c.md])
	if m.view.DocID() != docIDC {
		t.Fatalf("after clicking C: view=%d, want %d", m.view.DocID(), docIDC)
	}

	// Request A again — arms the deferral (A is still the saving target).
	m, cmd := m.requestOpenPath(docIDA, m.opentabs.PathAt(0))
	m = drainCmd(m, cmd)
	if !m.pendingReopen.active || m.pendingReopen.docID != docIDA {
		t.Fatalf("expected a deferred reopen for A, got %+v", m.pendingReopen)
	}

	// A genuinely new navigation (a third, never-opened file) must supersede
	// the stale deferral.
	dir := filepath.Dir(m.opentabs.PathAt(0))
	pathD := filepath.Join(dir, "d.md")
	if err := os.WriteFile(pathD, []byte("d-v1"), 0o644); err != nil {
		t.Fatal(err)
	}
	m, cmd = m.requestOpenPath(0, pathD)
	m = drainCmd(m, cmd)
	if m.pendingReopen.active {
		t.Fatalf("a new navigation request must supersede (clear) the stale deferral, got %+v", m.pendingReopen)
	}

	// Settle A's save. The superseded deferral must NOT replay — the user
	// stays on D, not yanked back to A.
	m, cmd = m.Update(fsMsg)
	m = drainCmd(m, cmd)
	if m.view.Path() != pathD {
		t.Fatalf("superseded deferral replayed anyway: view=%q, want %q (D)", m.view.Path(), pathD)
	}
}

// TestProbeResult_SuppressedWhileSavingSameTarget mirrors
// TestUpdateStatOnFlush_SuppressedWhileSaveInFlight for the probe-result path
// (WP5: stat-on-focus is now free via Load's own SyncState; probeDocCmd
// covers the idle-detection paths — dirChangedMsg/fileChangedMsg/flush-tick):
// a probe result for the exact document our own interactive save is writing
// must not raise a false "changed on disk" hint.
func TestProbeResult_SuppressedWhileSavingSameTarget(t *testing.T) {
	m := newTestWorkspace(t)
	m = loadFile(m, "note.md", "hello")
	docID := m.view.DocID()

	m.activeSave = SaveIdentity{RequestID: "x", InFlight: true, Path: "note.md", DocID: docID}

	m, _ = m.handleProbeResult(probeResultMsg{
		docID: docID, path: "note.md",
		state: docstate.SyncState{Kind: docstate.SyncDiskAhead},
	})
	if m.diskChangedHint {
		t.Fatal("expected diskChangedHint suppressed for our own in-flight save target")
	}
}

// TestProbeResult_StillFiresForUnrelatedTabDuringOtherSave proves the fix is
// targeted (savingTarget), not a blanket activeSave.InFlight check: a probe
// result for the CURRENTLY DISPLAYED doc while a DIFFERENT tab's save is in
// flight must still fire.
func TestProbeResult_StillFiresForUnrelatedTabDuringOtherSave(t *testing.T) {
	m := newTestWorkspace(t)
	m = loadFile(m, "note.md", "hello")
	docID := m.view.DocID()

	// A DIFFERENT file/doc is the one being saved.
	m.activeSave = SaveIdentity{RequestID: "x", InFlight: true, Path: "other.md", DocID: docID + 1}

	m, _ = m.handleProbeResult(probeResultMsg{
		docID: docID, path: "note.md",
		state: docstate.SyncState{Kind: docstate.SyncDiskAhead},
	})
	if !m.diskChangedHint {
		t.Fatal("expected diskChangedHint to fire for an unrelated tab while a different save is in flight")
	}
}
