//go:build !fuzzing

package workspace

import (
	"fmt"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/docstate"
	"rune/pkg/vfs"
)

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
