package workspace

import (
	"fmt"
	"time"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/docstate"
	"rune/pkg/vfs"
)

// openStore is a function-var indirection over openStoreCmd (called at the
// workspace.go Init call site) so DisableStoreOpenForTesting can swap it for
// a no-op — no unit test calls Init() directly (all inject StoreReadyMsg),
// so the swapped-out real disk open never runs a test into a real SQLite
// file at whatever path happens to be current.
var openStore = openStoreCmd

// openStoreMemory mirrors openStore for the WithMemoryStore ("None" chooser
// option) path — same function-var indirection, same DisableStoreOpenForTesting
// coverage.
var openStoreMemory = openStoreMemoryCmd

// DisableStoreOpenForTesting replaces openStore and openStoreMemory with a
// no-op (returns a nil Cmd) for the remainder of the process. Exported from
// a regular (non-_test.go) file — mirrors footer.DisableTimersForTesting
// (footer_testing.go) — so an importing package's test suite (e.g.
// internal/fuzz/harness) can silence it too; production code never calls
// this.
func DisableStoreOpenForTesting() {
	openStore = func(vfs.FS, string) tea.Cmd { return nil }
	openStoreMemory = func(vfs.FS) tea.Cmd { return nil }
}

// openStoreCmd opens the durable store at workDir/.rune/rune.db and wires it
// to fsys — the SAME vfs.FS value the workspace itself uses (§1.4.9 / S6:
// one injected filesystem, shared, never two independent nil-defaults to
// vfs.Disk{}). Per-workspace stores mean different workDirs never share a
// database and never contend; every Open failure now degrades in place
// (generic ErrMsg) — there is no hard-fail/quit path left (§12 Standing
// Decisions: "Per-workspace store, SQLite-native concurrency").
func openStoreCmd(fsys vfs.FS, workDir string) tea.Cmd {
	return func() tea.Msg {
		store, warn, err := docstate.Open(workDir)
		if err != nil {
			return ErrMsg{Err: fmt.Errorf("open storage: %w", err)}
		}
		store.UseFS(fsys)
		return StoreReadyMsg{Store: store, Warning: warn}
	}
}

// openStoreMemoryCmd opens the store as :memory: (docstate.OpenInMemory) —
// no workDir, no .rune directory, nothing survives past this process. Wired
// to fsys exactly like openStoreCmd so the store and workspace still share
// one injected filesystem for the file operations (Load/Probe/Materialize)
// that DO still hit real disk when the user explicitly saves. No Warning on
// success — this is a deliberate user choice, not the degraded fallback, and
// must not read as one.
func openStoreMemoryCmd(fsys vfs.FS) tea.Cmd {
	return func() tea.Msg {
		store, err := docstate.OpenInMemory(time.Now)
		if err != nil {
			return ErrMsg{Err: fmt.Errorf("open in-memory storage: %w", err)}
		}
		store.UseFS(fsys)
		return StoreReadyMsg{Store: store}
	}
}
