//go:build fuzzing

package workspace_test

import (
	"testing"
	"time"

	"rune/internal/fuzz/driver"
	"rune/internal/fuzz/workflow"
	"rune/pkg/command"
	"rune/pkg/docstate"
	"rune/pkg/editor/keybind"
	"rune/pkg/terminal"
	"rune/pkg/ui/keymap"
	"rune/pkg/ui/pages/workspace"
	"rune/pkg/ui/styles"
	"rune/pkg/vfs"
)

// humanPaths is the fixed set of virtual paths the human fuzzer can read,
// write, and navigate. RunHuman maps KindExternalWrite.PathIndex modulo
// len(humanPaths) to one of these paths.
var humanPaths = []string{
	"/fuzz/a.md",
	"/fuzz/b.md",
	"/fuzz/notes/c.md",
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
	// --- Seeds ---

	// Seed: global-seq dirty bug spec (Goal 4).
	// Cluster 7 = globalSeqDirtySpec: edit A, open B, edit B×2, undo×2.
	// TR-dirty-clear must hold after undoing all B edits.
	f.Add([]byte{7})

	// Seed: external-change conflict (EXT-NOCLOBBER).
	// Cluster 5 = externalChange: write file A externally, inject dir-changed,
	// then type + ⌘S — must surface FileSaveErrorMsg{Conflict:true}.
	f.Add([]byte{5, 0, 0}) // pathIndex=0 (a.md), watchSub=0 (dir-changed)
	f.Add([]byte{5, 0, 1}) // pathIndex=0 (a.md), watchSub=1 (read-error)

	// Seed: open in-file search and navigate results.
	// Cluster 0: ^F → type "hello" → FindNext×1 → Esc.
	// Encoding: clusterID=0, then data=[4,0,0,'h','e','l','l','o']
	//   queryLen=4%16+1=5, nexts=0%4+1=1, prevs=0%4=0, query="hello" (5 bytes @ offset 3).
	f.Add([]byte{0, 4, 0, 0, 'h', 'e', 'l', 'l', 'o'})

	// Seed: navigate filetree and open a file.
	// Cluster 1: ^x → Down×2 → Enter (opens b.md or notes/).
	f.Add([]byte{1, 2})

	// Seed: edit, undo, redo, save.
	// Cluster 2: ^e → "world" → undo×1 → redo×1 → ⌘S.
	f.Add([]byte{2, 5, 1, 1, 'w', 'o', 'r', 'l', 'd'})

	// Seed: tab churn — open 2 new files, pin, close 1.
	// Cluster 3: ^n×2 → ^p → ^w×1.
	f.Add([]byte{3, 2, 1})

	// Seed: dirty-close guard — type + ^w + save.
	// Cluster 4: ^e → "dirty" → ^w → 's' (save guard response).
	f.Add([]byte{4, 0})

	// Seed: dirty-close guard — discard.
	f.Add([]byte{4, 1})

	// Seed: resize then edit.
	// Cluster 6: resize to (40,15) then (120,40).
	f.Add([]byte{6, 40, 15, 120, 40})

	// Seed: search + edit combo.
	// Cluster 0 (search) followed by cluster 2 (edit+save).
	f.Add([]byte{
		0, 3, 1, 0, 'f', 'o', 'o', // OpenSearchAndFind: query "foo", FindNext×1
		2, 5, 2, 0, 'h', 'e', 'l', 'l', 'o', // EditUndoRedoSave: "hello", undo×2, save
	})

	// Seed: navigate to b.md then trigger external-change on it.
	f.Add([]byte{
		1, 2, // NavigateTreeAndOpen: Down×3 → Enter
		5, 1, 0, // ExternalChange: pathIndex=1 (b.md), watchSub=0
	})

	// Seed: follow a link — descend to a link line in a.md, Enter (follow), drain guard.
	// Cluster 8 = followLink: ^e → Down×k → Home → Enter → s/d/Esc.
	f.Add([]byte{8, 2, 0}) // down×3 (→ [B](b.md)), guard=save
	f.Add([]byte{8, 4, 1}) // down×5 (→ [x](missing.md) → LinkMissing), guard=discard
	f.Add([]byte{8, 5, 2}) // down×6 (→ external link), guard=cancel

	// Seed: dirty the open doc, then follow a link (eviction guard reachable), then save.
	f.Add([]byte{
		2, 5, 0, 0, 'd', 'i', 'r', 't', 'y', // EditUndoRedoSave: type "dirty", undo×1, no redo, ⌘S
		8, 2, 0, // FollowLink: follow [B](b.md), guard=save
	})

	// Seed: trash b.md from the explorer — confirm (cluster 9: ^x → Down×2 → ⌦ → y).
	// Cluster 9 consumes 2 bytes: data[0]=downs, data[1]=guard-response (0=y, 1=Esc).
	f.Add([]byte{9, 1, 0})
	// Seed: trash b.md — cancel via Esc.
	f.Add([]byte{9, 1, 1})

	// Seed: create a new file (^n) while focused in the explorer (cluster 10).
	// ^x → Down×2 → ^n → ^e.
	f.Add([]byte{10, 1})

	// Seed: make active doc dirty (cancel ^w), then try to trash it from the explorer.
	// Cluster 4 (Esc=2) → cluster 9 (Down×1 = a.md, the dirty active doc).
	// TRASH-DIRTY-BLOCK must hold: entry must not be removed; no guard raised.
	// Guard-response byte (1=Esc) is harmless noise since dirty-active shows an error.
	f.Add([]byte{4, 2, 9, 0, 1})

	// Seed: trash b.md (confirm), then create a new file from the explorer.
	// Cluster 9 consumes 2 bytes (data[0]=1→downs=2, data[1]=0→confirm);
	// cluster 10 gets its own byte (data[0]=0→downs=1).
	f.Add([]byte{9, 1, 0, 10, 0})

	f.Fuzz(func(t *testing.T, data []byte) {
		events := workflow.DecodeWorkflow(data)

		mem := seedMem()

		store, err := docstate.OpenInMemory(time.Now)
		if err != nil {
			t.Fatal(err)
		}
		defer store.Close()
		store.UseFS(mem)

		keys := keymap.Default()
		st := styles.Default()
		reg := command.NewBuilder().Build()
		res, _ := keybind.NewResolver(nil)
		caps := terminal.TermCaps{}

		m := workspace.New(keys, st, reg, res, caps, "/fuzz", []string{"/fuzz/a.md"}).WithFS(mem)

		if violation, _, _ := driver.RunHuman(m, events, store, mem, humanPaths, 80, 24); violation != nil {
			t.Errorf("invariant %s: %s", violation.InvariantID, violation.Message)
		}
	})
}
