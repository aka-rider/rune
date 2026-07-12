package workspace_test

import (
	"testing"
	"time"

	"rune/internal/editortest"
	"rune/internal/fuzz/driver"
	"rune/internal/fuzz/workflow"
	"rune/pkg/docstate"
	"rune/pkg/terminal"
	"rune/pkg/ui/keymap"
	"rune/pkg/ui/pages/workspace"
	"rune/pkg/ui/styles"
	"rune/pkg/vfs"
)

// humanPaths is the fixed set of virtual paths the human fuzzer can read,
// write, and navigate. RunHuman maps KindExternalWrite.PathIndex modulo
// len(humanPaths) to one of these paths. WP4 appends d.md (CRLF/no-trailing-
// newline byte-hostile — a §1.4.5 verbatim probe) and notes/e.md (a rename/
// delete target for the externalRename/externalDelete clusters) — appended,
// not inserted, so existing PathIndex 0/1/2 seeds keep their meaning.
var humanPaths = []string{
	"/fuzz/a.md",
	"/fuzz/b.md",
	"/fuzz/notes/c.md",
	"/fuzz/d.md",
	"/fuzz/notes/e.md",
}

// seedMem returns a vfs.Mem pre-seeded with humanPaths so the filetree and
// file-load machinery see real entries on bootstrap.
//
// The files are CROSS-LINKED (a↔b, both → notes/c, c → ../a) and salted with dead
// links (missing targets) and external schemes (http/mailto). Each link sits at the
// start of its own line so the followLink cluster (Home → Enter) lands the caret
// inside the link span. With the VFS-aware resolver (§1.4.9) these resolve against
// THIS Mem — the same backend the workspace loads from — so the follow path
// exercises LinkInternal, LinkMissing, and LinkExternal, not just "missing".
func seedMem() *vfs.Mem {
	mem := vfs.NewMem()
	_ = mem.WriteFile("/fuzz/a.md", []byte(
		"# File A\n\n[B](b.md)\n[notes](notes/c.md)\n[x](missing.md)\n"+
			"[web](https://example.com)\n[mail](mailto:a@b.com)\n"), 0o644)
	_ = mem.WriteFile("/fuzz/b.md", []byte(
		"# File B\n\n[A](a.md)\n[c](notes/c.md)\n[gone](../gone.md)\n"), 0o644)
	_ = mem.WriteFile("/fuzz/notes/c.md", []byte(
		"# Notes\n\n[A](../a.md)\n[none](none.md)\n"), 0o644)
	// d.md: deliberately byte-hostile — CRLF line endings and no trailing
	// newline, seeded verbatim (§1.4.5) so LOAD-VERBATIM/SAVE-VERBATIM can
	// catch a silent CRLF→LF or missing/added-trailing-newline normalization.
	_ = mem.WriteFile("/fuzz/d.md", []byte(
		"# D\r\nCRLF line\r\nlast line no eol"), 0o644)
	// e.md: a plain rename/delete target for externalRename/externalDelete.
	_ = mem.WriteFile("/fuzz/notes/e.md", []byte(
		"# E\n\nplain target file\n"), 0o644)
	return mem
}

// FuzzHumanSession runs the "human is working" cluster fuzzer.
// Instead of spraying individual keystrokes, corpus bytes are decoded by
// workflow.DecodeWorkflow into coherent multi-step clusters (open search,
// navigate tree, edit+undo, external change, etc.) so the fuzzer explores
// realistic user flows.
//
// The session uses a fully in-memory VFS for deterministic, disk-free runs.
// KindExternalWrite events mutate the Mem directly (advancing its mod-clock)
// so the §1.4.7 save-divergence guard (EXT-NOCLOBBER) is reachable; the
// RELOAD-NOMUT invariant is exercised by KindWatch(dir-changed).
func FuzzHumanSession(f *testing.F) {
	keys := keymap.Default()
	reg, res, err := driver.BuildFuzzApp(keys)
	if err != nil {
		f.Fatalf("BuildFuzzApp: %v", err)
	}

	// --- Seeds ---
	//
	// Sourced from the shared workflowSeeds table (scenario_test.go) so
	// FuzzHumanSession and TestWorkflowClusters can never drift apart — one
	// list of scenarios, explored by the fuzzer AND deterministically
	// re-checked by `make test` on every run.
	for _, seed := range workflowSeeds {
		f.Add(seed.data)
	}

	f.Fuzz(func(t *testing.T, data []byte) {
		events := workflow.DecodeWorkflow(data)
		// Bound per-exec wall-clock: every event runs a real cgo/SQLite
		// journal round-trip plus (flushDelay=0) a full-content snapshot, so
		// pathological long inputs (~300+ events of steady typing) took >5s
		// per exec and tripped the fuzz coordinator's worker-hang kill as
		// flaky "hung or terminated unexpectedly" failures. Truncation (not
		// skip) keeps the prefix coverage of long inputs and keeps existing
		// corpus entries valid; median inputs are far shorter and unaffected.
		const maxHumanEvents = 160
		if len(events) > maxHumanEvents {
			events = events[:maxHumanEvents]
		}

		mem := seedMem()

		store, err := docstate.OpenInMemory(editortest.AutoClock(time.Millisecond))
		if err != nil {
			t.Fatal(err)
		}
		defer store.Close()
		store.UseFS(mem)

		st := styles.Default()
		caps := terminal.TermCaps{}

		m := workspace.New(keys, st, reg, res, caps, "/fuzz", []string{"/fuzz/a.md"}).WithFS(mem).WithWatcher(workspace.NoopWatcher{})

		if violation, _, _ := driver.RunHuman(m, events, store, mem, humanPaths, 80, 24); violation != nil {
			t.Errorf("invariant %s: %s", violation.InvariantID, violation.Message)
		}
	})
}
