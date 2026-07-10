//go:build fuzzing

package workspace_test

// FuzzTwoSessionsSharedDoc is the two-editors-session-scoped-journal plan's
// Verification item 6: a NEW cluster driving two INDEPENDENT
// workspace.Model/docstate.Store pairs — two real rune windows — against
// ONE shared document, asserting no cross-session corruption and exercising
// at least one [M]erge resolution end-to-end.
//
// Unlike every other fuzzer in this package (FuzzSession, FuzzHumanSession,
// etc.), this ALWAYS backs both stores with a REAL temp directory via
// docstate.OpenAt (never OpenInMemory): the property under test is
// fundamentally about TWO SEPARATE *sql.DB connections sharing one real
// rune.db and its `sessions` table, which OpenInMemory's private
// per-connection :memory: database cannot reproduce. The two stores share a
// single vfs.Mem for the DOCUMENT's own content (so both see the same
// "disk"), exactly like the existing fuzzers already do for a single store.
//
// This is a deliberately SEPARATE, purpose-built harness rather than a new
// numbered entry in the existing single-model cluster grammar
// (internal/fuzz/workflow) — that grammar and its driver
// (internal/fuzz/driver.Run) are built around exactly ONE workspace.Model,
// and retrofitting a "which of two models" dimension into the shared
// grammar/corpus would risk remapping every existing cluster's seed corpus
// (see the project's own fuzz-corpus convention: grammar changes remap
// local corpus). The fixed script shape below (A edits, B edits, B saves, A
// edits more, A saves, guard resolved by a fuzzed choice) mirrors the
// existing mergeResolve cluster's own r%4 response-selection idiom
// (internal/fuzz/workflow/workflow_clusters_wp4.go) for consistency, while
// the actual TEXT each session types is fuzzed content — so this still
// explores real input diversity, not just one fixed scenario.
import (
	"bytes"
	"path/filepath"
	"reflect"
	"strings"
	"testing"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/command"
	"rune/pkg/docstate"
	"rune/pkg/editor/keybind"
	"rune/pkg/terminal"
	"rune/pkg/ui/components/footer"
	"rune/pkg/ui/components/markdownedit"
	"rune/pkg/ui/keymap"
	"rune/pkg/ui/pages/workspace"
	"rune/pkg/ui/styles"
	"rune/pkg/vfs"
)

// cmdSliceType/asCmdSlice/drainAll mirror internal/fuzz/driver.go's own
// drainCmd exactly (BatchMsg + the unexported tea.Sequence "sequenceMsg"
// reflection trick) — reimplemented locally because this file, like
// session_fuzz_test.go/human_fuzz_test.go, is package workspace_test and
// driver's own drainCmd is unexported. No invariant-checking here (unlike
// driver.go's version) — this harness's OWN cross-session assertions run
// once at the end of each fuzz iteration, below.
var twoSessionCmdSliceType = reflect.TypeOf([]tea.Cmd(nil))

func twoSessionAsCmdSlice(msg tea.Msg) ([]tea.Cmd, bool) {
	if msg == nil {
		return nil, false
	}
	if batch, ok := msg.(tea.BatchMsg); ok {
		return []tea.Cmd(batch), true
	}
	rv := reflect.ValueOf(msg)
	if rv.IsValid() && rv.Type().ConvertibleTo(twoSessionCmdSliceType) {
		return rv.Convert(twoSessionCmdSliceType).Interface().([]tea.Cmd), true
	}
	return nil, false
}

func drainAll(m workspace.Model, cmd tea.Cmd) workspace.Model {
	if cmd == nil {
		return m
	}
	msg := cmd()
	if msg == nil {
		return m
	}
	if cmds, ok := twoSessionAsCmdSlice(msg); ok {
		for _, c := range cmds {
			if c == nil {
				continue
			}
			m = drainAll(m, c)
		}
		return m
	}
	var next tea.Cmd
	m, next = m.Update(msg)
	return drainAll(m, next)
}

// takeChunk reads a length-prefixed (first byte = length, capped at 12),
// printable-ASCII-filtered chunk from data — excluding '@' specifically, so
// the fixed '@'-delimited markers typeMarked prepends can never collide
// with fuzzed content by coincidence (which would make a cross-session
// containment assertion below a false positive). Returns the chunk and the
// unconsumed remainder of data.
func takeChunk(data []byte) (string, []byte) {
	if len(data) == 0 {
		return "", nil
	}
	n := int(data[0])
	if n > 12 {
		n = 12
	}
	data = data[1:]
	if n > len(data) {
		n = len(data)
	}
	raw := data[:n]
	rest := data[n:]
	var b []byte
	for _, c := range raw {
		if c >= '!' && c <= '~' && c != '@' {
			b = append(b, c)
		}
	}
	return string(b), rest
}

// typeMarked types marker+suffix as literal keystrokes (one tea.KeyPressMsg
// per rune, draining inline — exactly how a real terminal delivers
// keystrokes), so a fresh AppendEdit lands per character. marker is a fixed
// '@'-delimited string (never present in suffix, by takeChunk's own
// filter) — assertions below check for the MARKER specifically, so a
// coincidental substring match between two independently fuzzed suffixes
// can never produce a false cross-session-corruption report.
func typeMarked(m workspace.Model, marker, suffix string) workspace.Model {
	for _, r := range marker + suffix {
		var cmd tea.Cmd
		m, cmd = m.Update(tea.KeyPressMsg{Code: r, Text: string(r)})
		m = drainAll(m, cmd)
	}
	return m
}

const (
	markerA1 = "@A1@" // A's first edit
	markerB1 = "@B1@" // B's edit
	markerA2 = "@A2@" // A's second edit, journaled AFTER B's save lands
)

func FuzzTwoSessionsSharedDoc(f *testing.F) {
	keys := keymap.Default()
	st := styles.Default()
	builder := command.NewBuilder()
	builder, err := markdownedit.RegisterCommands(builder)
	if err != nil {
		f.Fatalf("register commands: %v", err)
	}
	reg := builder.Build()
	res, _ := keybind.NewResolver(nil)
	caps := terminal.TermCaps{}

	// Seeds: [lenA, textA..., lenB, textB..., lenA2, textA2..., resolution]
	// resolution%4 picks [M]erge/[D]iscard/[S]ave-anyway/Cancel, mirroring
	// mergeResolve's own r%4 idiom.
	f.Add([]byte{6, 'a', '-', 'e', 'd', 'i', 't', 6, 'b', '-', 'e', 'd', 'i', 't', 5, 'm', 'o', 'r', 'e', 'A', 0})
	f.Add([]byte{6, 'a', '-', 'e', 'd', 'i', 't', 6, 'b', '-', 'e', 'd', 'i', 't', 5, 'm', 'o', 'r', 'e', 'A', 1})
	f.Add([]byte{6, 'a', '-', 'e', 'd', 'i', 't', 6, 'b', '-', 'e', 'd', 'i', 't', 5, 'm', 'o', 'r', 'e', 'A', 2})
	f.Add([]byte{6, 'a', '-', 'e', 'd', 'i', 't', 6, 'b', '-', 'e', 'd', 'i', 't', 5, 'm', 'o', 'r', 'e', 'A', 3})
	f.Add([]byte{0, 0, 0, 0}) // degenerate: every chunk empty

	f.Fuzz(func(t *testing.T, data []byte) {
		textA, rest := takeChunk(data)
		textB, rest2 := takeChunk(rest)
		textA2, rest3 := takeChunk(rest2)
		var resolution byte
		if len(rest3) > 0 {
			resolution = rest3[0]
		}

		dir := t.TempDir()
		path := filepath.Join(dir, "shared.md")
		mem := vfs.NewMem()
		if err := mem.WriteFile(path, []byte("shared baseline\n"), 0o644); err != nil {
			t.Fatalf("seed file: %v", err)
		}

		storeA, warnA, err := docstate.OpenAt(dir)
		if err != nil {
			t.Fatalf("OpenAt A: %v", err)
		}
		defer storeA.Close()
		storeA.UseFS(mem)
		storeB, warnB, err := docstate.OpenAt(dir)
		if err != nil {
			t.Fatalf("OpenAt B: %v", err)
		}
		defer storeB.Close()
		storeB.UseFS(mem)
		if warnA != "" || warnB != "" {
			t.Skip("degraded store — not the property under test")
		}

		mA := workspace.New(keys, st, reg, res, caps, dir, []string{path}).WithFS(mem).WithWatcher(workspace.NoopWatcher{})
		mB := workspace.New(keys, st, reg, res, caps, dir, []string{path}).WithFS(mem).WithWatcher(workspace.NoopWatcher{})

		var cmd tea.Cmd
		mA, cmd = mA.Update(tea.WindowSizeMsg{Width: 80, Height: 24})
		mA = drainAll(mA, cmd)
		mA = drainAll(mA, mA.Init())
		mA, cmd = mA.Update(workspace.StoreReadyMsg{Store: storeA})
		mA = drainAll(mA, cmd)

		mB, cmd = mB.Update(tea.WindowSizeMsg{Width: 80, Height: 24})
		mB = drainAll(mB, cmd)
		mB = drainAll(mB, mB.Init())
		mB, cmd = mB.Update(workspace.StoreReadyMsg{Store: storeB})
		mB = drainAll(mB, cmd)

		docID := mA.FuzzInspect().DocID
		if docID == 0 || mB.FuzzInspect().DocID == 0 {
			t.Skip("file did not resolve to a real doc")
		}
		if docID != mB.FuzzInspect().DocID {
			t.Fatalf("two sessions on the SAME file resolved DIFFERENT docIDs: %d vs %d", docID, mB.FuzzInspect().DocID)
		}

		// Focus the editor pane (ctrl+e) — a fresh workspace defaults focus
		// elsewhere (the file tree), and a keypress that never reaches the
		// editor is silently swallowed rather than inserted.
		mA, cmd = mA.Update(tea.KeyPressMsg{Code: 'e', Mod: tea.ModCtrl})
		mA = drainAll(mA, cmd)
		mB, cmd = mB.Update(tea.KeyPressMsg{Code: 'e', Mod: tea.ModCtrl})
		mB = drainAll(mB, cmd)
		if !mA.FuzzInspect().Focused || !mB.FuzzInspect().Focused {
			t.Fatalf("editor pane not focused after ctrl+e: A=%v B=%v", mA.FuzzInspect().Focused, mB.FuzzInspect().Focused)
		}

		mA = typeMarked(mA, markerA1, textA)
		mB = typeMarked(mB, markerB1, textB)

		// B saves.
		mB, cmd = mB.Update(tea.KeyPressMsg{Code: 's', Mod: tea.ModSuper})
		mB = drainAll(mB, cmd)

		// A types more AFTER B's save landed — the ordering that broke Sync
		// pre-fix (root cause: ancestorAt's self-exclusion no longer applies
		// once A's own position advances past B's save).
		mA = typeMarked(mA, markerA2, textA2)

		diskBeforeASave, err := mem.ReadFile(path)
		if err != nil {
			t.Fatalf("read disk before A's save: %v", err)
		}

		// A saves — must raise the conflict guard (Sync now correctly
		// Diverged post-fix), never silently clobber B's already-saved
		// content with no error and no guard ever shown.
		mA, cmd = mA.Update(tea.KeyPressMsg{Code: 's', Mod: tea.ModSuper})
		mA = drainAll(mA, cmd)

		snapA := mA.FuzzInspect()
		if snapA.GuardVisible && snapA.GuardKind == footer.GuardMerge {
			switch resolution % 4 {
			case 0: // [M]erge
				mA, cmd = mA.Update(footer.DataLossGuardResponseMsg{Response: footer.DataLossMerge})
				mA = drainAll(mA, cmd)

				// THE property this cluster exists to prove (B1): the
				// resolver's OWN installed content (a marker buffer for a
				// real conflict, or the auto-merged text for a clean
				// 3-way merge) is built from the TRUE pre-divergence
				// ancestor — never a degenerate ancestor==theirs collapse
				// that would silently discard one side with zero conflict
				// markers shown. Checked at the install itself (not after
				// an interactive O/N/T resolve+re-save, which needs no
				// re-verification here — mergemode's own package tests
				// already cover block-acceptance mechanics; this plan
				// touches Sync/ancestorAt, not mergemode's key handling).
				installed := mA.FuzzInspect().Content
				if !strings.Contains(installed, markerB1) {
					t.Fatalf("MERGE SILENTLY DROPPED B's change: installed content = %q, want it to contain B's marker %q", installed, markerB1)
				}
				if !strings.Contains(installed, markerA1) {
					t.Fatalf("MERGE SILENTLY DROPPED A's own change: installed content = %q, want it to contain A's marker %q", installed, markerA1)
				}
			case 1: // [D]iscard — buffer becomes theirs (B's already-saved
				// content); nothing new is written since theirs already IS
				// what's on disk. A's own work stays safe in ITS OWN
				// session/journal (this script never inspects that here;
				// TestTwoSessions_SameDoc_IsolatedUndoRedoAndDivergence
				// covers session-journal survival directly).
				mA, cmd = mA.Update(footer.DataLossGuardResponseMsg{Response: footer.DataLossDiscard})
				mA = drainAll(mA, cmd)
				finalDisk, err := mem.ReadFile(path)
				if err != nil {
					t.Fatalf("read final disk (discard): %v", err)
				}
				if !bytes.Contains(finalDisk, []byte(markerB1)) {
					t.Fatalf("after [D]iscard, disk lost B's already-saved content: %q", finalDisk)
				}
			case 2: // [S]ave anyway — an EXPLICIT, user-chosen force-write of
				// A's live buffer; B's change is legitimately superseded by
				// this choice, never silently.
				mA, cmd = mA.Update(footer.DataLossGuardResponseMsg{Response: footer.DataLossSaveAnyway})
				mA = drainAll(mA, cmd)
				finalDisk, err := mem.ReadFile(path)
				if err != nil {
					t.Fatalf("read final disk (save anyway): %v", err)
				}
				if !bytes.Contains(finalDisk, []byte(markerA1)) {
					t.Fatalf("after [S]ave anyway, disk does not contain A's own content: %q", finalDisk)
				}
			default: // Cancel — nothing written; disk stays exactly as it
				// was the instant before A's refused save attempt.
				mA, cmd = mA.Update(footer.DataLossGuardResponseMsg{Response: footer.DataLossCancel})
				mA = drainAll(mA, cmd)
				finalDisk, err := mem.ReadFile(path)
				if err != nil {
					t.Fatalf("read final disk (cancel): %v", err)
				}
				if !bytes.Equal(finalDisk, diskBeforeASave) {
					t.Fatalf("after Cancel, disk changed with no write ever confirmed: before=%q after=%q", diskBeforeASave, finalDisk)
				}
			}
		} else {
			// No guard was raised — this must mean A's attempted save
			// genuinely never diverged from B's (e.g. one side typed nothing).
			// Disk must then be either unchanged from immediately before A's
			// save, OR A's own save legitimately committed a superset that
			// still contains B's own marker (never a state where B's
			// marker vanished with no guard EVER having been shown).
			finalDisk, err := mem.ReadFile(path)
			if err != nil {
				t.Fatalf("read final disk (no-guard path): %v", err)
			}
			bStillPresentOnDisk := bytes.Contains(diskBeforeASave, []byte(markerB1))
			if bStillPresentOnDisk && !bytes.Equal(diskBeforeASave, finalDisk) && !bytes.Contains(finalDisk, []byte(markerB1)) {
				t.Fatalf("A's save changed disk WITHOUT ever raising a conflict guard, and B's marker is now gone: before=%q after=%q", diskBeforeASave, finalDisk)
			}
		}

		// CROSS-SESSION ISOLATION (root cause #1/#2): B's OWN session-scoped
		// journal reconstruction must never contain EITHER of A's markers —
		// B never resolved anything against A's content in this script, so
		// any trace of A's edits in B's own reconstruction is corruption.
		bOwn, err := storeB.RecoverDocument(docID)
		if err != nil {
			t.Fatalf("storeB.RecoverDocument: %v", err)
		}
		if strings.Contains(bOwn, markerA1) {
			t.Fatalf("CROSS-SESSION CORRUPTION: B's own journal reconstruction contains A's first edit: %q", bOwn)
		}
		if strings.Contains(bOwn, markerA2) {
			t.Fatalf("CROSS-SESSION CORRUPTION: B's own journal reconstruction contains A's second edit: %q", bOwn)
		}
	})
}
