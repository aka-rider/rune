package workspace_test

// Phase 2 of the QA-rehaul plan: a deterministic scenario suite that shares
// its seed corpus with FuzzHumanSession, so `make test` deterministically
// re-checks every one of the 20 workflow clusters (plus fixed-permutation
// race scenarios) with the exact same invariant arsenal the fuzzer uses —
// one source of scenarios, no drift between the two QA pillars.
//
// This file is `package workspace_test` (external): it imports
// rune/internal/fuzz/driver, which imports rune/pkg/ui/pages/workspace — an
// internal (package workspace) test file importing driver would be the same
// import-cycle shape harness.go's doc comment describes. The four existing
// workspace fuzz-test files (session/human/two_sessions/corpus_repro) are
// already external, matching.

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
)

// workflowSeeds is the single source of FuzzHumanSession's f.Add seeds AND
// TestWorkflowClusters's deterministic subtests — extracted verbatim from
// the byte-strings that used to live inline in FuzzHumanSession (one source,
// no drift). Each entry's name documents which of the 20 workflow clusters
// (workflow.DecodeWorkflow) it drives and why; see workflow.go's grammar
// comment for the cluster ID → decoder mapping.
var workflowSeeds = []struct {
	name string
	data []byte
}{
	// Cluster 7 globalSeqDirtySpec: edit A, open B, edit B×2, undo×2.
	// TR-dirty-clear must hold after undoing all B edits.
	{"globalSeqDirtySpec/goal4", []byte{7}},

	// Cluster 5 externalChange: write file A externally, inject dir-changed,
	// then type + ⌘S — must surface FileSaveErrorMsg{Conflict:true}.
	{"externalChange/dirChanged", []byte{5, 0, 0}},     // pathIndex=0 (a.md), watchSub=0 (dir-changed)
	{"externalChange/readError", []byte{5, 0, 1}},      // pathIndex=0 (a.md), watchSub=1 (read-error)
	{"externalChange/inPlaceChanged", []byte{5, 0, 2}}, // pathIndex=0 (a.md), watchSub=2 (in-place file-changed, BUG1)

	// Cluster 0: ^F → type "hello" → FindNext×1 → Esc.
	// Encoding: clusterID=0, then data=[4,0,0,'h','e','l','l','o']
	//   queryLen=4%16+1=5, nexts=0%4+1=1, prevs=0%4=0, query="hello" (5 bytes @ offset 3).
	{"search/openAndFind", []byte{0, 4, 0, 0, 'h', 'e', 'l', 'l', 'o'}},

	// Cluster 1: ^x → Down×2 → Enter (opens b.md or notes/).
	{"filetree/navigateAndOpen", []byte{1, 2}},

	// Cluster 2: ^e → "world" → undo×1 → redo×1 → ⌘S.
	{"edit/undoRedoSave", []byte{2, 5, 1, 1, 'w', 'o', 'r', 'l', 'd'}},

	// Cluster 3: ^n×2 → ^p → ^w×1.
	{"tabs/churnPinClose", []byte{3, 2, 1}},

	// Cluster 4: ^e → "dirty" → ^w → 's' (save guard response).
	{"dirtyClose/save", []byte{4, 0}},
	// Cluster 4: discard variant.
	{"dirtyClose/discard", []byte{4, 1}},

	// Cluster 6: resize to (40,15) then (120,40).
	{"resize/thenEdit", []byte{6, 40, 15, 120, 40}},

	// Cluster 0 (search) followed by cluster 2 (edit+save).
	{"search/thenEditCombo", []byte{
		0, 3, 1, 0, 'f', 'o', 'o', // OpenSearchAndFind: query "foo", FindNext×1
		2, 5, 2, 0, 'h', 'e', 'l', 'l', 'o', // EditUndoRedoSave: "hello", undo×2, save
	}},

	// Navigate to b.md then trigger external-change on it.
	{"filetree/navigateThenExternalChange", []byte{
		1, 2, // NavigateTreeAndOpen: Down×3 → Enter
		5, 1, 0, // ExternalChange: pathIndex=1 (b.md), watchSub=0
	}},

	// Cluster 8 followLink: ^e → Down×k → Home → Enter → s/d/Esc.
	{"followLink/internalSave", []byte{8, 2, 0}},   // down×3 (→ [B](b.md)), guard=save
	{"followLink/missingDiscard", []byte{8, 4, 1}}, // down×5 (→ [x](missing.md) → LinkMissing), guard=discard
	{"followLink/externalCancel", []byte{8, 5, 2}}, // down×6 (→ external link), guard=cancel

	// Dirty the open doc, then follow a link (eviction guard reachable), then save.
	{"edit/dirtyThenFollowLink", []byte{
		2, 5, 0, 0, 'd', 'i', 'r', 't', 'y', // EditUndoRedoSave: type "dirty", undo×1, no redo, ⌘S
		8, 2, 0, // FollowLink: follow [B](b.md), guard=save
	}},

	// Cluster 9 (2 bytes: downs, guard-response 0=y/1=Esc): trash b.md.
	{"trash/confirm", []byte{9, 1, 0}},
	{"trash/cancel", []byte{9, 1, 1}},

	// Cluster 10: create a new file (^n) while focused in the explorer.
	// ^x → Down×2 → ^n → ^e.
	{"filetree/createNewFile", []byte{10, 1}},

	// Make active doc dirty (cancel ^w), then try to trash it from the
	// explorer. TRASH-DIRTY-BLOCK must hold: entry must not be removed; no
	// guard raised. Guard-response byte (1=Esc) is harmless noise since
	// dirty-active shows an error.
	{"trash/dirtyActiveBlocked", []byte{4, 2, 9, 0, 1}},

	// Trash b.md (confirm), then create a new file from the explorer.
	{"trash/thenCreateNewFile", []byte{9, 1, 0, 10, 0}},

	// WP4 seeds — clusters 11-15 (numClusters is now 20).

	// Cluster 11 mergeResolve: edit ours, external write, ⌘S → refused →
	// GuardMerge. r%4 selects the response; u is the undo count (merge
	// branch only).
	{"mergeResolve/merge-undo0", []byte{11, 0, 0}}, // r=0 [M]erge, u=0 → resolver keys, persist via ⌘S
	{"mergeResolve/merge-undo1", []byte{11, 0, 1}}, // r=0 [M]erge, u=1 → undo unwinds into the resolver, drain re-raised guard
	{"mergeResolve/merge-undo2", []byte{11, 0, 2}}, // r=0 [M]erge, u=2 → more undo steps
	{"mergeResolve/discard", []byte{11, 1, 0}},     // r=1 [D]iscard → handleDataLossDiscardConflict/applyDiscardConflict
	{"mergeResolve/saveAnyway", []byte{11, 2, 0}},  // r=2 [S]ave anyway → handleDataLossSaveAnyway (CAS force-write)
	{"mergeResolve/cancel", []byte{11, 3, 0}},      // r=3 Esc → Cancel

	// Cluster 12 externalRename [src]: rename a tracked file to d.md
	// externally, watch fires, letter-jump-and-open d.md — RenamedFrom
	// branch of handleFileLoadedMsg.
	{"externalRename/fromA", []byte{12, 0}}, // a.md → d.md
	{"externalRename/fromB", []byte{12, 1}}, // b.md → d.md

	// Cluster 13 externalDelete [_,r,v]: v=0 watch-detect (idle probe →
	// GuardDeleted), v=1 save-detect (⌘S → FileSaveErrorMsg{Missing} →
	// GuardDeleted). r selects s/d/Esc.
	{"externalDelete/watchDetectSave", []byte{13, 0, 0, 0}},   // v=0 watch-detect, r=0 [S]ave (recreate)
	{"externalDelete/saveDetectDiscard", []byte{13, 0, 1, 1}}, // v=1 save-detect, r=1 [D]iscard (purge)

	// Cluster 14 evictionPressure [n,v]: v=0 dirty-victim
	// (GuardDirty(evict)), v=1 no-eligible-victim refusal (pin a.md, 10
	// untitleds, tree-open refused).
	{"eviction/dirtyVictimSave", []byte{14, 0, 0}},    // v=0 dirty-victim, response=0%3=0 [S]ave → evictSave/evictSaveAck
	{"eviction/dirtyVictimDiscard", []byte{14, 1, 0}}, // v=0 dirty-victim, response=1%3=1 [D]iscard → evictDiscard
	{"eviction/noEligibleVictim", []byte{14, 1, 1}},   // v=1 no-eligible-victim refusal

	// Cluster 15 quitSaveAll [r]: dirty two tabs → KindQuitRequest →
	// GuardDirty(quit).
	{"quit/saveAll", []byte{15, 0}},    // r=0 [S]ave all → saveAllDirtyForQuit + teardownAndQuit
	{"quit/discardAll", []byte{15, 1}}, // r=1 [D]iscard all → immediate teardownAndQuit
	{"quit/cancel", []byte{15, 2}},     // r=2 Esc → cancel, quit aborted

	// Cluster 16 selectionClipboard [op,t]: select → action → undo → save.
	{"selection/copyShiftWordCJK", []byte{16, 0, 2}},    // op=0 Copy, t=2 (ShiftWordRight selection, CJK text)
	{"selection/cutSelectAll", []byte{16, 1, 0}},        // op=1 Cut + KindClipboard paste-over, t=0 (SelectAll)
	{"selection/moveLineShiftRight", []byte{16, 2, 1}},  // op=2 MoveLineUp/Down, t=1 (Shift+Right x3 selection)
	{"selection/addCursorMultiPaste", []byte{16, 3, 2}}, // op=3 AddCursorBelow + multi-line paste distribute + Esc

	// Cluster 17 unicodeTyping [t1,t2,k]: paste → multi-byte keys →
	// Left/Backspace → paste.
	{"unicode/asciiSingleCJK", []byte{17, 1, 0, 0}},    // t1=ascii, t2=empty, k=0 → 1 multi-byte key (CJK)
	{"unicode/cjkMathAlnumCycle", []byte{17, 2, 6, 5}}, // t1=CJK, t2=math-alnum, k=5 → 6 multi-byte keys (cycles all 3)

	// Cluster 18 dictationCluster [t,v]: seed → ^V start → dictation
	// events, v%4 scenario.
	{"dictation/happyPath", []byte{18, 1, 0}},          // v=0 happy path
	{"dictation/emptyResetHazard", []byte{18, 1, 1}},   // v=1 empty-reset hazard — the fixed FinalTranscriptionMsg bug
	{"dictation/staleTicket", []byte{18, 1, 2}},        // v=2 stale-ticket (^N invalidates the session mid-flight)
	{"dictation/transientThenFatal", []byte{18, 1, 3}}, // v=3 transient-then-fatal ErrorMsg

	// Cluster 19 workspaceChrome [d]: TabSwitch/Pin/Zen/Help/Chat/chord/scroll.
	{"chrome/tabSwitch0", []byte{19, 0}}, // TabSwitch(0)
	{"chrome/tabSwitch5", []byte{19, 5}}, // TabSwitch(5)

	// Cross-cluster seed: externalChange (cluster 5) leaves a divergence
	// undetected, then mergeResolve (cluster 11) layers its OWN conflict on
	// top — exercises the two clusters' interaction, not just either alone.
	{"crossCluster/externalChangeThenMergeResolve", []byte{5, 0, 0, 11, 0, 0}},
}

// TestWorkflowClusters runs every workflowSeeds entry as a deterministic
// subtest through the exact same driver.RunHuman path FuzzHumanSession uses
// — the fuzzer explores new byte sequences; this test deterministically
// re-checks the ones already known to matter, on every `make test` run.
func TestWorkflowClusters(t *testing.T) {
	keys := keymap.Default()
	reg, res, err := driver.BuildFuzzApp(keys)
	if err != nil {
		t.Fatalf("BuildFuzzApp: %v", err)
	}

	for _, seed := range workflowSeeds {
		t.Run(seed.name, func(t *testing.T) {
			t.Parallel()

			events := workflow.DecodeWorkflow(seed.data)

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
				t.Fatalf("invariant %s: %s", violation.InvariantID, violation.Message)
			}
		})
	}
}
