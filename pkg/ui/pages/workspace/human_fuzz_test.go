//go:build fuzzing

package workspace_test

import (
	"testing"
	"time"

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

	// Seed: global-seq dirty bug spec (Goal 4).
	// Cluster 7 = globalSeqDirtySpec: edit A, open B, edit B×2, undo×2.
	// TR-dirty-clear must hold after undoing all B edits.
	f.Add([]byte{7})

	// Seed: external-change conflict (EXT-NOCLOBBER).
	// Cluster 5 = externalChange: write file A externally, inject dir-changed,
	// then type + ⌘S — must surface FileSaveErrorMsg{Conflict:true}.
	f.Add([]byte{5, 0, 0}) // pathIndex=0 (a.md), watchSub=0 (dir-changed)
	f.Add([]byte{5, 0, 1}) // pathIndex=0 (a.md), watchSub=1 (read-error)
	f.Add([]byte{5, 0, 2}) // pathIndex=0 (a.md), watchSub=2 (in-place file-changed, BUG1)

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

	// WP4 seeds — clusters 11-15 (numClusters is now 20).

	// Cluster 11 mergeResolve: edit ours, external write, ⌘S → refused → GuardMerge.
	// r%4 selects the response; u is the undo count (merge branch only).
	f.Add([]byte{11, 0, 0}) // r=0 [M]erge, u=0 → resolver keys, persist via ⌘S
	f.Add([]byte{11, 0, 1}) // r=0 [M]erge, u=1 → undo unwinds into the resolver, drain re-raised guard
	f.Add([]byte{11, 0, 2}) // r=0 [M]erge, u=2 → more undo steps
	f.Add([]byte{11, 1, 0}) // r=1 [D]iscard → handleDataLossDiscardConflict/applyDiscardConflict
	f.Add([]byte{11, 2, 0}) // r=2 [S]ave anyway → handleDataLossSaveAnyway (CAS force-write)
	f.Add([]byte{11, 3, 0}) // r=3 Esc → Cancel

	// Cluster 12 externalRename [src]: rename a tracked file to d.md externally,
	// watch fires, letter-jump-and-open d.md — RenamedFrom branch of handleFileLoadedMsg.
	f.Add([]byte{12, 0}) // a.md → d.md
	f.Add([]byte{12, 1}) // b.md → d.md

	// Cluster 13 externalDelete [_,r,v]: v=0 watch-detect (idle probe → GuardDeleted),
	// v=1 save-detect (⌘S → FileSaveErrorMsg{Missing} → GuardDeleted). r selects s/d/Esc.
	f.Add([]byte{13, 0, 0, 0}) // v=0 watch-detect, r=0 [S]ave (recreate)
	f.Add([]byte{13, 0, 1, 1}) // v=1 save-detect, r=1 [D]iscard (purge)

	// Cluster 14 evictionPressure [n,v]: v=0 dirty-victim (GuardDirty(evict)),
	// v=1 no-eligible-victim refusal (pin a.md, 10 untitleds, tree-open refused).
	f.Add([]byte{14, 0, 0}) // v=0 dirty-victim, response=0%3=0 [S]ave → evictSave/evictSaveAck
	f.Add([]byte{14, 1, 0}) // v=0 dirty-victim, response=1%3=1 [D]iscard → evictDiscard
	f.Add([]byte{14, 1, 1}) // v=1 no-eligible-victim refusal

	// Cluster 15 quitSaveAll [r]: dirty two tabs → KindQuitRequest → GuardDirty(quit).
	f.Add([]byte{15, 0}) // r=0 [S]ave all → saveAllDirtyForQuit + teardownAndQuit
	f.Add([]byte{15, 1}) // r=1 [D]iscard all → immediate teardownAndQuit
	f.Add([]byte{15, 2}) // r=2 Esc → cancel, quit aborted

	// Cluster 16 selectionClipboard [op,t]: select → action → undo → save.
	f.Add([]byte{16, 0, 2}) // op=0 Copy, t=2 (ShiftWordRight selection, CJK text)
	f.Add([]byte{16, 1, 0}) // op=1 Cut + KindClipboard paste-over, t=0 (SelectAll)
	f.Add([]byte{16, 2, 1}) // op=2 MoveLineUp/Down, t=1 (Shift+Right x3 selection)
	f.Add([]byte{16, 3, 2}) // op=3 AddCursorBelow + multi-line paste distribute + Esc

	// Cluster 17 unicodeTyping [t1,t2,k]: paste → multi-byte keys → Left/Backspace → paste.
	f.Add([]byte{17, 1, 0, 0}) // t1=ascii, t2=empty, k=0 → 1 multi-byte key (CJK)
	f.Add([]byte{17, 2, 6, 5}) // t1=CJK, t2=math-alnum, k=5 → 6 multi-byte keys (cycles all 3)

	// Cluster 18 dictationCluster [t,v]: seed → ^V start → dictation events, v%4 scenario.
	f.Add([]byte{18, 1, 0}) // v=0 happy path
	f.Add([]byte{18, 1, 1}) // v=1 empty-reset hazard — the fixed FinalTranscriptionMsg bug
	f.Add([]byte{18, 1, 2}) // v=2 stale-ticket (^N invalidates the session mid-flight)
	f.Add([]byte{18, 1, 3}) // v=3 transient-then-fatal ErrorMsg

	// Cluster 19 workspaceChrome [d]: TabSwitch/Pin/Zen/Help/Chat/chord/scroll.
	f.Add([]byte{19, 0}) // TabSwitch(0)
	f.Add([]byte{19, 5}) // TabSwitch(5)

	// Cross-cluster seed: externalChange (cluster 5) leaves a divergence
	// undetected, then mergeResolve (cluster 11) layers its OWN conflict on
	// top — exercises the two clusters' interaction, not just either alone.
	f.Add([]byte{5, 0, 0, 11, 0, 0})

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

		store, err := docstate.OpenInMemory(time.Now)
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
