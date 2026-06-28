package workspace

// Tests for the non-destructive pending-load gate (one load chokepoint via
// beginLoad). The invariant under test: the editor buffer and its identity
// fields (filePath/docID/baseline) change ONLY together, on a successful load.
// A failed load is a no-op on state, so it never strands the editor blank and
// ⌘S can never write an empty buffer over a real file (CLAUDE.md §0.1 rung 1).

import (
	"os"
	"strings"
	"testing"

	tea "charm.land/bubbletea/v2"
	"charm.land/lipgloss/v2"

	"rune/pkg/ui/help"
	"rune/pkg/vfs"
)

// openReal drives a real load of path through the chokepoint against fsys,
// draining the loadFileCmd so FileLoadedMsg/FileLoadErrorMsg is applied.
func openReal(m Model, path string) Model {
	var cmd tea.Cmd
	m, cmd = m.requestOpenPath(0, path)
	for _, msg := range execCmds(cmd) {
		m, _ = m.Update(msg)
	}
	return m
}

// T1 — a failed load preserves the previous, fully-consistent document.
func TestPendingLoad_FailedLoadPreservesPreviousDoc(t *testing.T) {
	m := loadFile(newTestWorkspace(t), "a.md", "ALPHA")
	wantPath, wantContent := m.view.Path(), m.editor.Content()
	wantBaseline := m.view.Baseline()

	// Switch to a different path; arm the gate but don't deliver the result yet.
	m, _ = m.requestOpenPath(0, "b.md")
	if !m.pendingLoad.active || m.pendingLoad.path != "b.md" {
		t.Fatalf("gate not armed: %+v", m.pendingLoad)
	}

	// The load fails (matching gen, so it settles the awaited load).
	m, _ = m.Update(FileLoadErrorMsg{Path: "b.md", Err: errTest, Gen: m.loadGen})

	if m.pendingLoad.active {
		t.Error("gate not cleared after load error")
	}
	if m.editor.Content() != wantContent {
		t.Errorf("buffer changed on failed load: %q want %q", m.editor.Content(), wantContent)
	}
	if m.view.Path() != wantPath {
		t.Errorf("filePath changed on failed load: %q want %q", m.view.Path(), wantPath)
	}
	if m.view.Baseline() != wantBaseline {
		t.Error("baseline changed on failed load")
	}
}

// T2 — ⌘S after a failed load does NOT write the blank buffer over the previous
// real file (the rung-1 regression test). Under the old SetContent("") code the
// editor was blank, so this would have written "" over "REAL".
func TestPendingLoad_SaveAfterFailedLoadDoesNotClobber(t *testing.T) {
	mem := vfs.NewMem()
	if err := mem.WriteFile("a.md", []byte("REAL"), 0o644); err != nil {
		t.Fatal(err)
	}
	m := newTestWorkspace(t).WithFS(mem)

	m = openReal(m, "a.md")
	if m.editor.Content() != "REAL" || m.view.Path() != "a.md" {
		t.Fatalf("setup load failed: content=%q path=%q", m.editor.Content(), m.view.Path())
	}

	// Open a path missing from mem → FileLoadErrorMsg.
	m = openReal(m, "missing.md")
	if m.view.Path() != "a.md" || m.editor.Content() != "REAL" {
		t.Fatalf("failed load mutated state: content=%q path=%q", m.editor.Content(), m.view.Path())
	}

	// Panic-save now: must write the preserved "REAL", never "".
	var scmd tea.Cmd
	m, scmd = m.startSave()
	for _, msg := range execCmds(scmd) {
		m, _ = m.Update(msg)
	}
	got, err := mem.ReadFile("a.md")
	if err != nil {
		t.Fatal(err)
	}
	if string(got) != "REAL" {
		t.Fatalf("a.md clobbered: %q want %q", got, "REAL")
	}
}

// T3 — ⌘S is inert while a load is pending (the buffer may not yet match the
// incoming identity).
func TestPendingLoad_SaveInertWhilePending(t *testing.T) {
	m := loadFile(newTestWorkspace(t), "a.md", "ALPHA")
	m, _ = m.requestOpenPath(0, "b.md") // gate armed, result not delivered

	_, cmd := m.startSave()
	if cmd != nil {
		t.Fatal("startSave must be inert while a load is pending")
	}
}

// T4 — a failed close→neighbour load lands on the save-safe transitional identity
// (0/""), never blank-buffer + neighbour-path.
func TestPendingLoad_FailedCloseNeighbourIsSaveSafe(t *testing.T) {
	m := loadFile(newTestWorkspace(t), "a.md", "ALPHA")
	m = loadFile(m, "b.md", "BETA") // active b.md, tabs {a, b}

	var cmd tea.Cmd
	m, cmd = m.executeClose(m.view.DocID(), m.view.Path()) // close b → neighbour a
	_ = cmd
	if !m.pendingLoad.active || m.pendingLoad.path != "a.md" {
		t.Fatalf("close did not arm the neighbour load: %+v", m.pendingLoad)
	}
	if m.view.Path() != "" || m.view.DocID() != 0 {
		t.Fatalf("identity not transitional after close: path=%q docID=%d", m.view.Path(), m.view.DocID())
	}

	// The neighbour load fails.
	m, _ = m.Update(FileLoadErrorMsg{Path: m.pendingLoad.path, Err: errTest, Gen: m.pendingLoad.gen})

	if m.view.Path() != "" || m.view.DocID() != 0 {
		t.Fatalf("not save-safe after failed neighbour load: path=%q docID=%d", m.view.Path(), m.view.DocID())
	}
	if _, cmd := m.startSave(); cmd != nil {
		t.Error("⌘S must be a no-op on the transitional untitled identity")
	}
}

// T5 — the anti-flash is preserved non-destructively: the previous content does
// not flash while pending, the buffer stays intact, the view is pure, and the
// gate frame is the same height as the real frame (no pane jump).
func TestPendingLoad_AntiFlashPreservedNoHeightJump(t *testing.T) {
	// A leading sentinel char absorbs the cursor cell (cursor resets to offset 0
	// after a load, where the editor reverse-styles the cell), so the marker
	// substring stays contiguous in the rendered view and Contains is reliable.
	m := focusEditor(loadFile(newTestWorkspace(t), "a.md", "Zmarker-alpha"))

	m, _ = m.requestOpenPath(0, "b.md") // arm the gate; don't deliver
	if !m.pendingLoad.active {
		t.Fatal("gate not armed")
	}
	if m.editor.Content() != "Zmarker-alpha" {
		t.Errorf("buffer destroyed while pending: %q", m.editor.Content())
	}

	v1 := m.View().Content
	v2 := m.View().Content
	if v1 != v2 {
		t.Error("View() not pure while pending")
	}
	if strings.Contains(v1, "marker-alpha") {
		t.Error("previous content flashed during pending load")
	}
	if h1, h2 := lipgloss.Height(m.editor.RenderEmpty()), lipgloss.Height(m.editor.View()); h1 != h2 {
		t.Errorf("RenderEmpty height %d != View height %d — pane would jump", h1, h2)
	}

	m, _ = m.Update(FileLoadedMsg{Path: "b.md", Content: []byte("Zmarker-beta"), Gen: m.loadGen})
	if m.pendingLoad.active {
		t.Error("gate not cleared after successful load")
	}
	if m.editor.Content() != "Zmarker-beta" {
		t.Errorf("loaded content not applied: %q", m.editor.Content())
	}
	if !strings.Contains(m.View().Content, "marker-beta") {
		t.Error("loaded content not shown after load (gate not lifted)")
	}
}

// T6 — during a close transition the incoming tab is marked active even though
// the live identity is still the transitional 0/"" (the intentional one-hop lead;
// INV-ACTIVE-SYNC holds only after settle).
func TestPendingLoad_TabSetLeadsDuringCloseTransition(t *testing.T) {
	m := loadFile(newTestWorkspace(t), "a.md", "ALPHA")
	m = loadFile(m, "b.md", "BETA")

	m, _ = m.executeClose(m.view.DocID(), m.view.Path()) // close b → neighbour a pending
	gen := m.pendingLoad.gen
	m, _ = m.finalize(nil) // finalize is where TAB-SET happens

	if got := m.opentabs.ActiveHandle(); got.Path != "a.md" {
		t.Fatalf("incoming tab not active during transition: ActiveHandle=%+v", got)
	}
	if m.view.Path() != "" || m.view.DocID() != 0 {
		t.Fatalf("identity should still be transitional, got path=%q docID=%d", m.view.Path(), m.view.DocID())
	}

	m, _ = m.Update(FileLoadedMsg{Path: "a.md", Content: []byte("ALPHA"), Gen: gen})
	m, _ = m.finalize(nil)
	if m.view.Path() != "a.md" {
		t.Fatalf("identity not synced after load: %q", m.view.Path())
	}
	if got := m.opentabs.ActiveHandle(); got.Path != "a.md" {
		t.Fatalf("active handle not synced after load: %+v", got)
	}
}

// T7 — a failed NON-close load keeps the current tab active throughout (the
// scoped finalize uses live identity when it is non-empty).
func TestPendingLoad_FailedNonCloseLoadKeepsCurrentTab(t *testing.T) {
	m := loadFile(newTestWorkspace(t), "a.md", "ALPHA")

	m, _ = m.requestOpenPath(0, "b.md") // filetree-click style; live identity still a.md
	gen := m.pendingLoad.gen
	m, _ = m.finalize(nil)
	if got := m.opentabs.ActiveHandle(); got.Path != "a.md" {
		t.Fatalf("current tab not kept active during pending: %+v", got)
	}

	m, _ = m.Update(FileLoadErrorMsg{Path: "b.md", Err: errTest, Gen: gen})
	m, _ = m.finalize(nil)
	if got := m.opentabs.ActiveHandle(); got.Path != "a.md" {
		t.Fatalf("current tab not restored after failed load: %+v", got)
	}
}

// T8 — the placeholder-discard predicate now reads the untitled's REAL content:
// an empty startup untitled is discarded on open; a non-empty scratch is kept.
func TestPendingLoad_PlaceholderDiscardOnlyWhenEmpty(t *testing.T) {
	mem := vfs.NewMem()
	if err := mem.WriteFile("real.md", []byte("Y"), 0o644); err != nil {
		t.Fatal(err)
	}

	t.Run("empty untitled discarded", func(t *testing.T) {
		m := newTestWorkspace(t).WithFS(mem) // one empty untitled tab
		m = openReal(m, "real.md")
		if m.opentabs.HasUntitledPlaceholder() {
			t.Error("empty untitled placeholder should be discarded on open")
		}
		if m.opentabs.Len() != 1 {
			t.Errorf("want 1 tab (real.md), got %d", m.opentabs.Len())
		}
	})

	t.Run("non-empty untitled kept", func(t *testing.T) {
		m := focusEditor(newTestWorkspace(t).WithFS(mem))
		m = typeChar(m, 'x') // untitled now holds "x"
		m = openReal(m, "real.md")
		if m.opentabs.Len() != 2 {
			t.Errorf("non-empty scratch tab should be kept: want 2 tabs, got %d", m.opentabs.Len())
		}
	})
}

// T10 — a synchronous transition (new untitled, help, untitled tab) taken while
// a load is in flight clears the gate, so its content is never masked by a blank
// frame. Without the clear the center pane would stay blank until the stale async
// load resolved.
func TestPendingLoad_SyncTransitionClearsGate(t *testing.T) {
	t.Run("CreateUntitled clears gate", func(t *testing.T) {
		m := loadFile(newTestWorkspace(t), "a.md", "ALPHA")
		m, _ = m.requestOpenPath(0, "b.md") // arm the gate
		if !m.pendingLoad.active {
			t.Fatal("gate not armed")
		}
		m, _ = m.CreateUntitled()
		if m.pendingLoad.active {
			t.Error("CreateUntitled did not clear the pending-load gate")
		}
	})

	t.Run("help switch clears gate", func(t *testing.T) {
		m := loadFile(newTestWorkspace(t), "a.md", "ALPHA")
		m, _ = m.requestOpenPath(0, "b.md") // arm the gate
		m, _ = m.requestOpenPath(0, help.DocPath)
		if m.pendingLoad.active {
			t.Error("synchronous help switch did not clear the pending-load gate")
		}
	})
}

// T9 — structural: loadFileCmd has exactly two callers — beginLoad (interactive
// chokepoint) and Init (startup, which can't retain a beginLoad model so it issues
// reads with New-seeded generations). The load discipline still lives in one place
// per path: beginLoad for everything interactive.
func TestPendingLoad_LoadFileCmdHasTwoCallers(t *testing.T) {
	entries, err := os.ReadDir(".")
	if err != nil {
		t.Fatal(err)
	}
	callers := 0
	for _, e := range entries {
		name := e.Name()
		if !strings.HasSuffix(name, ".go") || strings.HasSuffix(name, "_test.go") {
			continue
		}
		b, err := os.ReadFile(name)
		if err != nil {
			t.Fatal(err)
		}
		for _, line := range strings.Split(string(b), "\n") {
			if strings.Contains(line, "loadFileCmd(") && !strings.Contains(line, "func loadFileCmd") {
				callers++
			}
		}
	}
	if callers != 2 {
		t.Fatalf("loadFileCmd must have exactly two callers (beginLoad, Init); found %d", callers)
	}
}

// ─────────────────────────────────────────────────────────────────────────────
// Load-generation correlation: the displayed document is ALWAYS the last one
// requested. A superseded / out-of-order read can never display the wrong doc.
// (The fuzz driver settles in issue order and can't reach these — deliver by hand.)
// ─────────────────────────────────────────────────────────────────────────────

// G1 (headline) — open A, then B; deliver B's result, THEN A's stale result last.
// B must remain displayed; the stale A is dropped.
func TestLoadGen_LatestWinsStaleDropped(t *testing.T) {
	m := loadFile(newTestWorkspace(t), "base.md", "BASE")

	m, _ = m.requestOpenPath(0, "a.md")
	genA := m.pendingLoad.gen
	m, _ = m.requestOpenPath(0, "b.md")
	genB := m.pendingLoad.gen
	if genA == genB {
		t.Fatalf("generations must differ: genA=%d genB=%d", genA, genB)
	}

	// B settles first (the awaited load).
	m, _ = m.Update(FileLoadedMsg{Path: "b.md", Content: []byte("BBB"), Gen: genB})
	// A's stale read arrives LAST — must NOT clobber B.
	m, _ = m.Update(FileLoadedMsg{Path: "a.md", Content: []byte("AAA"), Gen: genA})

	if m.view.Path() != "b.md" {
		t.Errorf("displayed filePath = %q, want b.md (stale A clobbered it)", m.view.Path())
	}
	if m.editor.Content() != "BBB" {
		t.Errorf("displayed content = %q, want BBB (stale A clobbered it)", m.editor.Content())
	}
}

// G2 — a stale read that arrives after a synchronous switch (CreateUntitled) is
// dropped: it must not overwrite the untitled the user switched to.
func TestLoadGen_StaleAfterSyncSwitchDropped(t *testing.T) {
	m := loadFile(newTestWorkspace(t), "base.md", "BASE")

	m, _ = m.requestOpenPath(0, "a.md")
	genA := m.pendingLoad.gen
	m, _ = m.CreateUntitled() // synchronous: supersedeLoad bumps the token

	m, _ = m.Update(FileLoadedMsg{Path: "a.md", Content: []byte("AAA"), Gen: genA})

	if m.view.Path() != "" {
		t.Errorf("stale read displayed over the untitled: filePath = %q", m.view.Path())
	}
	if m.editor.Content() == "AAA" {
		t.Error("stale read content displayed over the untitled")
	}
}

// G3 — two reads of the SAME path with different baselines (an external change
// between issues): the newer read's baseline must win, even if the older arrives
// last (the §1.4.7 stale-baseline window).
func TestLoadGen_StaleSamePathBaselineDropped(t *testing.T) {
	m := loadFile(newTestWorkspace(t), "a.md", "OLD")

	m, _ = m.requestOpenPath(0, "a.md") // first re-open
	gen1 := m.pendingLoad.gen
	// Force a second in-flight read of the SAME path (path==filePath would early
	// return, so supersede + re-arm directly via beginLoad).
	m, _ = m.beginLoad(0, "a.md")
	gen2 := m.pendingLoad.gen
	newBaseline := diskBaseline{size: 999, valid: true}

	// Newer read settles, then the older arrives last with a STALE baseline.
	m, _ = m.Update(FileLoadedMsg{Path: "a.md", Content: []byte("NEW"), Baseline: newBaseline, Gen: gen2})
	m, _ = m.Update(FileLoadedMsg{Path: "a.md", Content: []byte("OLD"), Baseline: diskBaseline{size: 1, valid: true}, Gen: gen1})

	if m.view.Baseline() != newBaseline {
		t.Errorf("stale baseline retained: got %+v want %+v", m.view.Baseline(), newBaseline)
	}
	if m.editor.Content() != "NEW" {
		t.Errorf("stale content retained: %q want NEW", m.editor.Content())
	}
}

// G4 — startup reads carry New-seeded generations; the last initial file is the
// displayed doc, and a later interactive switch is never clobbered by a residual
// startup read.
func TestLoadGen_StartupTokenAcceptedThenSuperseded(t *testing.T) {
	m := newWorkspaceWithFiles(t, "one.md", "two.md")

	// New seeded the overlay for the LAST initial file at gen == len(initialFiles).
	lastGen := m.pendingLoad.gen
	// The first startup read (gen 1) only opens a tab; the last (gen==lastGen) displays.
	m, _ = m.Update(FileLoadedMsg{Path: "one.md", Content: []byte("ONE"), Gen: 1})
	if m.view.Path() == "one.md" {
		t.Error("non-last startup read should not become the displayed doc")
	}
	m, _ = m.Update(FileLoadedMsg{Path: "two.md", Content: []byte("TWO"), Gen: lastGen})
	if m.view.Path() != "two.md" {
		t.Fatalf("last startup file should be displayed; filePath=%q", m.view.Path())
	}

	// An interactive switch gets a strictly greater gen; a residual startup read
	// (stale gen) can't clobber it.
	m, _ = m.requestOpenPath(0, "three.md")
	if m.pendingLoad.gen <= lastGen {
		t.Fatalf("interactive gen %d must exceed startup gen %d", m.pendingLoad.gen, lastGen)
	}
}
