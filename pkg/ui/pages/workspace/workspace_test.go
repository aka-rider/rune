package workspace

// Shared test constructors and low-level Cmd-draining helpers used across
// every *_test.go file in this package (§1.6 — the actual test CASES that
// used to live alongside these are split into workspace_open_test.go,
// workspace_saveident_test.go, workspace_layout_test.go,
// workspace_dirtyflag_test.go, and workspace_filedelete_test.go, keeping
// each file under the 500-LoC limit; this file now holds only what every
// other test file depends on).

import (
	"errors"
	"os"
	"path/filepath"
	"testing"

	tea "charm.land/bubbletea/v2"

	"rune/internal/editortest"
	"rune/internal/fuzz/session"
	"rune/pkg/command"
	"rune/pkg/docstate"
	"rune/pkg/editor/keybind"
	"rune/pkg/terminal"
	"rune/pkg/ui/components/markdownedit"
	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"
	"rune/pkg/vfs"
)

// TestMain lives in main_test.go (package workspace_test, external), not
// here: it calls internal/fuzz/harness.Hermetic(), which imports this
// package — a package-internal test file (package workspace, this one)
// importing anything that imports its own package is an import cycle Go
// rejects outright ("import cycle not allowed in test"). A package's test
// binary has exactly one TestMain regardless of how many internal/external
// test files it mixes, so main_test.go's TestMain governs every test in
// this package, internal or external.

// newTestWorkspace creates a sized workspace for testing with a file pre-loaded.
func newTestWorkspace(t *testing.T) Model {
	t.Helper()
	keys := keymap.Default()
	st := styles.Default()
	builder := command.NewBuilder()
	builder, err := markdownedit.RegisterCommands(builder)
	if err != nil {
		t.Fatalf("register commands: %v", err)
	}
	reg := builder.Build()
	res, _ := keybind.NewResolver(nil)

	m := New(keys, st, reg, res, terminal.TermCaps{}, "", nil).WithWatcher(NoopWatcher{})
	m, _ = m.Update(tea.WindowSizeMsg{Width: 80, Height: 24})
	return m
}

// newScrollWorkspace is like newTestWorkspace but wires the real keymap
// resolver, so editor navigation keys (arrows, page up/down) actually resolve
// to commands. Required for tests that exercise scrolling.
func newScrollWorkspace(t *testing.T) Model {
	t.Helper()
	keys := keymap.Default()
	st := styles.Default()
	builder := command.NewBuilder()
	builder, err := markdownedit.RegisterCommands(builder)
	if err != nil {
		t.Fatalf("register commands: %v", err)
	}
	reg := builder.Build()
	cmdBindings, err := keys.CommandBindings()
	if err != nil {
		t.Fatalf("command bindings: %v", err)
	}
	res, err := keybind.NewResolver(cmdBindings)
	if err != nil {
		t.Fatalf("new resolver: %v", err)
	}
	m := New(keys, st, reg, res, terminal.TermCaps{}, "", nil).WithWatcher(NoopWatcher{})
	m, _ = m.Update(tea.WindowSizeMsg{Width: 80, Height: 24})
	return m
}

// newWorkspaceWithFiles is newTestWorkspace but with initialFiles set, so New
// seeds the startup load overlay. Used to exercise the startup generation path.
func newWorkspaceWithFiles(t *testing.T, files ...string) Model {
	t.Helper()
	keys := keymap.Default()
	st := styles.Default()
	builder := command.NewBuilder()
	builder, err := markdownedit.RegisterCommands(builder)
	if err != nil {
		t.Fatalf("register commands: %v", err)
	}
	reg := builder.Build()
	res, _ := keybind.NewResolver(nil)
	m := New(keys, st, reg, res, terminal.TermCaps{}, "", files).WithWatcher(NoopWatcher{})
	m, _ = m.Update(tea.WindowSizeMsg{Width: 80, Height: 24})
	return m
}

// withStore wires a real, in-memory-backed VFS store into the workspace,
// exactly as the app does once StoreReadyMsg arrives. This upgrades the
// startup untitled to a durable VFS document (ensureScratchDoc), so untitled
// content survives tab switches via reconstruction rather than the old
// in-memory stash. Defaults the workspace (and, via WithFS's propagation, the
// store) to a fresh hermetic vfs.Mem when the caller hasn't already injected
// one — WP5's loadFile drives real store.Load/Materialize calls, so an
// un-wired nil-default to vfs.Disk{} would touch the real filesystem at
// whatever (often relative) path a test passes. A caller that wants real disk
// (a t.TempDir()-rooted test) still works: apply .WithFS(vfs.Disk{}) — or any
// other vfs.FS — BEFORE or AFTER calling withStore; WithFS propagates to
// m.store whenever it's already set, so the two can never strand on
// different backends regardless of call order.
func withStore(t *testing.T, m Model) Model {
	t.Helper()
	if m.fs == nil {
		m = m.WithFS(vfs.NewMem())
	}
	store := docstate.NewTestStore(t)
	store.UseFS(m.fsys())
	m, cmd := m.Update(StoreReadyMsg{Store: store})
	for _, msg := range execCmds(cmd) {
		m, _ = m.Update(msg)
	}
	return m
}

// loadFile simulates loading a file into the workspace through the real load
// generation handshake: arm the load (beginLoad, which stamps the current gen)
// then deliver the matching FileLoadedMsg, so the displayed-document gen gate
// accepts it. Delivering a raw FileLoadedMsg with no matching pending gen would
// be dropped (it would only open a tab), so all simulated loads must go through
// here. When a store is wired, drives the REAL store.Load(path) — writing
// content to the shared vfs.FS first, UNLESS the file already holds that exact
// content (a caller that pre-arranged the file itself, e.g. simulating an
// external rename): a redundant WriteFile would needlessly churn the inode
// (vfs.Disk's atomic write always creates a fresh one), destroying a
// same-inode invariant a test may be relying on — so tests exercise the actual
// production identity/history/SyncState path, not a hand-built stand-in. A
// path whose parent directory doesn't really exist (many tests use a
// throwaway "/fake/..." path with vfs.Disk{}, not caring whether it's real)
// makes the disk write/load fail — falls back to store.OpenPath, which is
// safe even for a nonexistent file (path-keyed identity, inode=0), so the
// test still gets a REAL, usable docID rather than stranding on a synthetic
// zero one. Without a store at all (the pre-store-ready race window some
// tests deliberately exercise), falls back to a synthetic LoadResult
// mirroring loadFileCmd's own store==nil branch.
func loadFile(m Model, path string, content string) Model {
	m, _ = m.beginLoad(0, path)
	var result docstate.LoadResult
	if m.store != nil {
		if dir := filepath.Dir(path); dir != "" && dir != "." {
			_ = m.fsys().MkdirAll(dir, 0o755) // fire-and-forget: test setup
		}
		needsWrite := true
		if existing, err := m.fsys().ReadFile(path); err == nil && string(existing) == content {
			needsWrite = false
		}
		writeErr := error(nil)
		if needsWrite {
			writeErr = m.fsys().WriteFile(path, []byte(content), 0o644)
		}
		if writeErr == nil {
			if loaded, err := m.store.Load(path); err == nil {
				result = loaded
			}
		}
		if result.DocID == 0 {
			if ref, err := m.store.OpenPath(path); err == nil {
				result = docstate.LoadResult{DocID: ref.ID, DiskContent: content, Recovered: content}
			}
		}
	} else {
		result = docstate.LoadResult{DiskContent: content, Recovered: content}
	}
	m, _ = m.Update(FileLoadedMsg{Path: path, Result: result, Gen: m.loadGen})
	return m
}

// resizeWorkspace returns a workspace sized to the given dimensions with both
// panes visible and at default widths. Width 100 is wide enough to satisfy the
// minCenterW=24 floor even with both panes at their defaults.
func resizeWorkspace(t *testing.T, w, h int) Model {
	t.Helper()
	keys := keymap.Default()
	st := styles.Default()
	reg := command.NewBuilder().Build()
	res, _ := keybind.NewResolver(nil)

	m := New(keys, st, reg, res, terminal.TermCaps{}, "", nil).WithWatcher(NoopWatcher{})
	m, _ = m.Update(tea.WindowSizeMsg{Width: w, Height: h})
	return m
}

// execCmds executes a tea.Cmd and collects all resulting messages.
// Thin package-local alias for editortest.ExecCmds (the single home of the
// expansion logic — handles nil cmds, tea.BatchMsg, AND tea.Sequence's
// unexported container), kept so the ~40 existing call sites in this package
// read unchanged.
func execCmds(cmd tea.Cmd) []tea.Msg {
	return editortest.ExecCmds(cmd)
}

// settle delivers cmd's message (and any message a resulting Cmd yields,
// recursively, breadth-first) through m.Update, fully settling an async round
// trip (e.g. keypress → footer response msg → materializeCmd → FileSavedMsg)
// within a single deterministic test step — no time.Sleep, no reliance on
// runtime scheduling. THE drain helper for this package: consolidates the
// five per-file variants that predated it (drainCmd, workspace_deleted's
// recursive settle, drainAll, drainUntilMarker → editortest.DrainUntil,
// settleOneHop → its documented one-hop loop in workspace_merge_modal_test.go).
//
// After the round trip settles, settle runs the fuzzer's full L0 invariant
// sweep (session.Check over m.FuzzInspect() — the SAME checkers
// internal/fuzz/driver runs after every fuzzed step), so every workspace test
// that settles a round trip is invariant-checked for free. Once per settle,
// not per delivered message: FuzzInspect snapshots the full cell grid, and
// per-message sweeping measured ~5x on this package's untagged suite (the
// plan's anticipated sampling valve; the fuzzing pillar still checks every
// single step). Calls session directly rather than invarianttest: this is a
// package-INTERNAL test file, and invarianttest imports workspace (import
// cycle); session/snapshot do not.
func settle(t *testing.T, m Model, cmd tea.Cmd) Model {
	t.Helper()
	m = editortest.Drain(m, cmd, Model.Update)
	if v := session.Check(m.FuzzInspect()); v != nil {
		t.Fatalf("invariant %s violated after settle: %s", v.InvariantID, v.Message)
	}
	return m
}

// withStoreAt is withStore over a REAL on-disk recovery store
// (docstate.OpenAt under dir) and real vfs.Disk — for tests that need
// persistence to survive a store close/reopen (e.g. §1.4.3 recovery across a
// process restart). The store is NOT auto-closed on cleanup: restart tests
// close it explicitly mid-test and reopen; OpenAt's session files live under
// the test's TempDir and vanish with it.
func withStoreAt(t *testing.T, m Model, dir string) Model {
	t.Helper()
	m = m.WithFS(vfs.Disk{})
	store, _, err := docstate.OpenAt(dir)
	if err != nil {
		t.Fatalf("docstate.OpenAt(%s): %v", dir, err)
	}
	store.UseFS(m.fsys())
	m, cmd := m.Update(StoreReadyMsg{Store: store})
	return settle(t, m, cmd)
}

// diskFixture creates a store-backed workspace over real files on disk
// (vfs.Disk, so every save genuinely churns the inode via Materialize's
// publish step, Exchange/RenameExcl — unlike vfs.Mem, which only assigns a
// new inode on a path's first write) and loads the named files as tabs in
// order, returning the workspace and each file's resolved docID.
func diskFixture(t *testing.T, contents map[string]string, order []string) (Model, map[string]int64) {
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

var errTest = errors.New("test error")

// focusEditor sets workspace focus to the editor pane and flushes the focus
// state to all children via a WindowSizeMsg. Required because applyFocus runs
// at the END of Update — a direct field assignment isn't visible to children
// until the next Update completes.
func focusEditor(m Model) Model {
	m.focus = paneCenter
	m, _ = m.Update(tea.WindowSizeMsg{Width: 80, Height: 24})
	return m
}
