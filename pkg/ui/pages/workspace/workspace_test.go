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
	"path/filepath"
	"testing"
	"time"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/command"
	"rune/pkg/docstate"
	"rune/pkg/editor/keybind"
	"rune/pkg/terminal"
	"rune/pkg/ui/components/markdownedit"
	"rune/pkg/ui/keymap"
	"rune/pkg/ui/styles"
	"rune/pkg/vfs"
)

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

	m := New(keys, st, reg, res, terminal.TermCaps{}, "", nil)
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
	m := New(keys, st, reg, res, terminal.TermCaps{}, "", nil)
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
	m := New(keys, st, reg, res, terminal.TermCaps{}, "", files)
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

	m := New(keys, st, reg, res, terminal.TermCaps{}, "", nil)
	m, _ = m.Update(tea.WindowSizeMsg{Width: w, Height: h})
	return m
}

// execCmds executes a tea.Cmd and collects all resulting messages.
// Handles nil cmds and tea.BatchMsg.
func execCmds(cmd tea.Cmd) []tea.Msg {
	if cmd == nil {
		return nil
	}
	msg := cmd()
	if msg == nil {
		return nil
	}
	if batch, ok := msg.(tea.BatchMsg); ok {
		var msgs []tea.Msg
		for _, c := range batch {
			msgs = append(msgs, execCmds(c)...)
		}
		return msgs
	}
	return []tea.Msg{msg}
}

// execFastCmds is execCmds for a Cmd batch that mixes fast, deterministic
// leaves (a probeDocCmd's single fsys.Stat/ReadFile) with the real directory
// watcher Cmd (workspace_watch.go's watchDirCmd) — dirChangedMsg's and
// fileChangedMsg's handlers both re-arm startWatch alongside issuing a probe.
// The real watcher blocks indefinitely on a live fsnotify channel with no
// timeout of its own; execCmds (or drainCmd, which calls it) would hang a
// test forever waiting for a filesystem event that never comes. Each leaf
// gets a short bounded window; a leaf that doesn't return in time (the
// watcher) is silently dropped rather than awaited — its goroutine is
// abandoned, which is fine in a short-lived test process.
func execFastCmds(cmd tea.Cmd) []tea.Msg {
	if cmd == nil {
		return nil
	}
	type result struct{ msg tea.Msg }
	ch := make(chan result, 1)
	go func() { ch <- result{cmd()} }()
	select {
	case r := <-ch:
		if r.msg == nil {
			return nil
		}
		if batch, ok := r.msg.(tea.BatchMsg); ok {
			var msgs []tea.Msg
			for _, c := range batch {
				msgs = append(msgs, execFastCmds(c)...)
			}
			return msgs
		}
		return []tea.Msg{r.msg}
	case <-time.After(200 * time.Millisecond):
		return nil // leaf never returned (the real fsnotify watcher) — drop it
	}
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
