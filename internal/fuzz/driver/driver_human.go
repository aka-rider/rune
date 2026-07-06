//go:build fuzzing

package driver

import (
	"errors"
	"fmt"

	tea "charm.land/bubbletea/v2"

	"rune/internal/fuzz/event"
	"rune/internal/fuzz/invariant"
	"rune/internal/fuzz/session"
	dictengine "rune/pkg/dictation"
	"rune/pkg/docstate"
	"rune/pkg/ui/components/footer"
	"rune/pkg/ui/components/textedit"
	pgworkspace "rune/pkg/ui/pages/workspace"
	"rune/pkg/vfs"
)

// RunReorderSaves exercises the edit-during-save race that the in-order drain
// structurally cannot reach: it DEFERS each async save result (FileSavedMsg) so
// subsequent fuzz events keep journaling edits while the save is "in flight", then
// delivers the deferred saves and asserts SAVE-RACE — if edits were journaled after
// a save's issue position, the saved doc MUST still be dirty after the save settles.
// A MarkSaved that stamps the live journal head (instead of the position the written
// bytes reflect) marks the doc clean despite unsaved edits — a §1.4.2/§1.4.8
// durability bug the inline fuzzer (which settles each save before the next event)
// never sees.
func RunReorderSaves(model pgworkspace.Model, events []event.Event, store *docstate.Store, mem *vfs.Mem, paths []string, w, h int) (*invariant.Violation, string, [][]textedit.Cell) {
	rs := &runState{store: store, monitors: session.NewMonitors(), baselines: map[int64]string{}, mem: mem, reorderSaves: true}

	var v *invariant.Violation
	model, v = bootstrap(rs, model, store, w, h)
	if v != nil {
		return v, rs.frozenFrame, rs.frozenCells
	}

	for _, ev := range events {
		switch ev.Kind {
		case event.KindExternalWrite:
			// Advance the Mem mod-clock (not the focus of this harness — EXT-NOCLOBBER
			// is covered elsewhere; here it only perturbs save divergence).
			if mem != nil && len(paths) > 0 {
				path := paths[int(ev.PathIndex)%len(paths)]
				content := ev.Text
				if content == "" {
					content = "external-write\n"
				}
				_ = mem.WriteFile(path, []byte(content), 0o644) // fire-and-forget: Mem never fails
			}
			continue
		default:
			msg := eventToMsg(ev)
			if msg == nil {
				continue
			}
			model, v = drainMsg(rs, model, msg)
		}
		if v != nil {
			return v, rs.frozenFrame, rs.frozenCells
		}
		if rs.quit {
			break // production has no "after quit" state to keep exercising
		}
	}

	// Deliver each deferred save (running MarkSaved) AFTER the interleaved edits, then
	// assert the durability invariant. reorderSaves stays true so the inline
	// TR-dirty-clear check (which expects a clean doc) is skipped here — SAVE-RACE
	// owns the correctly-dirty outcome.
	for _, ds := range rs.deferredSaves {
		model, v = drainMsg(rs, model, ds.msg)
		if v != nil {
			return v, rs.frozenFrame, rs.frozenCells
		}
		if rs.store == nil || ds.docID == 0 {
			continue
		}
		cur, cerr := rs.store.CurrentSeq(ds.docID)
		if cerr != nil || cur <= ds.issueSeq {
			continue // no edits interleaved while the save was in flight → nothing to assert
		}
		dirty, derr := rs.store.IsDirty(ds.docID)
		if derr != nil {
			continue
		}
		if !dirty {
			snap := model.FuzzInspect()
			return &invariant.Violation{
				InvariantID: "SAVE-RACE",
				Message: fmt.Sprintf(
					"doc %d marked clean after save settled, but %d edit(s) were journaled while the save was in flight (saved@seq=%d, head=%d) — unsaved edits silently lost (§1.4.2)",
					ds.docID, cur-ds.issueSeq, ds.issueSeq, cur,
				),
			}, snap.Frame, snap.Cells
		}
	}
	return nil, "", nil
}

// watchTargetPath resolves the path a KindWatch(file-changed) event names,
// indexing into paths ∪ {the active file}. The extra slot lets the fuzzer
// target the open document explicitly (mirroring WatchSub==1's convention of
// always targeting snap.ActiveFilePath); the rest target the same fixed
// background/decoy pool KindExternalWrite already uses, so both an in-place
// edit to the open file and to an unrelated background file are reachable.
func watchTargetPath(paths []string, active string, idx uint8) string {
	if len(paths) == 0 {
		return active
	}
	slot := int(idx) % (len(paths) + 1)
	if slot == len(paths) {
		return active
	}
	return paths[slot]
}

// RunHuman is the entry point for FuzzHumanSession. It extends Run with
// support for KindExternalWrite and KindWatch events, which require access to
// the shared vfs.Mem and the list of seeded file paths.
//
//   - KindExternalWrite: calls mem.WriteFile for paths[ev.PathIndex % len(paths)].
//     Advances Mem's mod-clock so the workspace's diskBaseline becomes stale;
//     the next ⌘S must surface FileSaveErrorMsg{Conflict:true} (§1.4.7).
//     No model message is injected. Adds the path to rs.externalWrites so the
//     EXT-NOCLOBBER monitor is armed both for the active file and background tabs.
//
//   - KindWatch(sub=0): injects workspace.FuzzDirChangedMsg() → workspace calls
//     reloadDirCmd → drains to filetree.DirReloadedMsg.
//
//   - KindWatch(sub=1): injects workspace.FuzzFileWatchReadErrorMsg() so the
//     read-error path is exercised.
//
//   - KindWatch(sub=2): injects workspace.FuzzFileChangedMsg(path) — the
//     in-place-external-write path (BUG1) that the inline fuzzer's binary
//     dispatch never reached. path is resolved by watchTargetPath: it may be
//     the active file or a background/decoy file, exercising both the
//     disk-changed-hint path and the InFlight/savingTarget suppression.
func RunHuman(model pgworkspace.Model, events []event.Event, store *docstate.Store, mem *vfs.Mem, paths []string, w, h int) (*invariant.Violation, string, [][]textedit.Cell) {
	rs := &runState{store: store, monitors: session.NewMonitors(), baselines: map[int64]string{}, mem: mem}

	var v *invariant.Violation
	model, v = bootstrap(rs, model, store, w, h)
	if v != nil {
		return v, rs.frozenFrame, rs.frozenCells
	}

	for _, ev := range events {
		switch ev.Kind {
		case event.KindExternalWrite:
			if mem == nil || len(paths) == 0 {
				continue
			}
			path := paths[int(ev.PathIndex)%len(paths)]
			content := ev.Text
			if content == "" {
				content = "external-write\n" // non-empty so baseline size diverges
			}
			// Arm EXT-NOCLOBBER only if this write actually changes the
			// file's bytes. A content-IDENTICAL rewrite (e.g. a second
			// externalChange cluster reusing the same fixed "external-
			// content\n" string after an earlier discard already adopted
			// exactly those bytes as ours) introduces nothing new for
			// production to protect — its content-hash-based CAS correctly
			// treats it as no divergence at all, so requiring a
			// FileSaveErrorMsg{Conflict}/reload to "reconcile" a no-op
			// write would be asserting a syntactic obligation production
			// was never semantically obligated to raise (the same
			// re-derives-production-dispatch failure mode the plan's
			// "Rejected" section retired the old DL3/extNoClobber msg→kind
			// map for).
			prevBytes, readErr := mem.ReadFile(path)
			changed := readErr != nil || string(prevBytes) != content
			_ = mem.WriteFile(path, []byte(content), 0o644) // fire-and-forget: Mem never fails
			if changed {
				// Covers both active and background tabs.
				if rs.externalWrites == nil {
					rs.externalWrites = make(map[string]struct{})
				}
				rs.externalWrites[path] = struct{}{}
			}
			continue

		case event.KindWatch:
			var msg tea.Msg
			switch ev.WatchSub {
			case 0:
				msg = pgworkspace.FuzzDirChangedMsg()
			case 1:
				snap := model.FuzzInspect()
				msg = pgworkspace.FuzzFileWatchReadErrorMsg(snap.ActiveFilePath, errors.New("watch: simulated read error"))
			default: // 2: in-place external write observed on a file in the watched dir
				snap := model.FuzzInspect()
				msg = pgworkspace.FuzzFileChangedMsg(watchTargetPath(paths, snap.ActiveFilePath, ev.PathIndex))
			}
			model, v = drainMsg(rs, model, msg)

		case event.KindExternalRename:
			if mem == nil || len(paths) == 0 {
				continue
			}
			snap := model.FuzzInspect()
			src := watchTargetPath(paths, snap.ActiveFilePath, ev.PathIndex)
			dst := watchTargetPath(paths, snap.ActiveFilePath, ev.DestIndex)
			if err := mem.Rename(src, dst); err == nil {
				// EXT-NOCLOBBER bookkeeping: the obligation moves WITH the
				// bytes, only if one existed — src no longer exists to be
				// silently clobbered, but the renamed file (now at dst)
				// still carries whatever unreconciled external change it
				// had. A rename of a file with no pending obligation must
				// not manufacture one for dst.
				if _, pending := rs.externalWrites[src]; pending {
					delete(rs.externalWrites, src)
					rs.externalWrites[dst] = struct{}{}
				}
			}
			continue

		case event.KindExternalRemove:
			if mem == nil || len(paths) == 0 {
				continue
			}
			snap := model.FuzzInspect()
			path := watchTargetPath(paths, snap.ActiveFilePath, ev.PathIndex)
			if err := mem.Remove(path); err == nil {
				// A removed path can never be silently clobbered again — and
				// leaving it armed would falsely trip EXT-NOCLOBBER on the
				// GuardDeleted [S]ave recreate path (a legitimate, guard-
				// confirmed recreation of a file that's simply gone, not an
				// overwrite of someone else's bytes).
				delete(rs.externalWrites, path)
			}
			continue

		case event.KindDictation:
			var msg tea.Msg
			switch ev.DictSub {
			case 0:
				msg = dictengine.PartialTranscriptionMsg{Accumulated: ev.Text}
			case 1:
				msg = dictengine.FinalTranscriptionMsg{Text: ev.Text}
			case 2:
				msg = dictengine.ErrorMsg{Err: errors.New("dictation: simulated transient error"), Fatal: false}
			default: // 3
				msg = dictengine.ErrorMsg{Err: errors.New("dictation: simulated fatal error"), Fatal: true}
			}
			model, v = drainMsg(rs, model, msg)

		case event.KindClipboard:
			model, v = drainMsg(rs, model, tea.ClipboardMsg{Content: ev.Text})

		case event.KindQuitRequest:
			model, v = drainMsg(rs, model, footer.ConfirmQuitMsg{})

		default:
			msg := eventToMsg(ev)
			if msg == nil {
				continue
			}
			model, v = drainMsg(rs, model, msg)
		}

		if v != nil {
			return v, rs.frozenFrame, rs.frozenCells
		}
		if rs.quit {
			// Production has no "after quit" state to keep exercising: a
			// completed quit (KindQuitRequest + quitSaveAll's [S]/[D]
			// response, now reachable end-to-end since the ^C^C chord is
			// otherwise structurally unreachable inline) closes the store
			// and would, in the real bubbletea runtime, end the program —
			// no further Msg is ever delivered. Continuing to feed script
			// events into a Model whose store is already closed (a later
			// trash-confirm, a stale FileLoadErrorMsg from a load issued
			// before quit finally settling, etc.) exercises undefined
			// territory, not a real production state.
			break
		}
	}
	return nil, "", nil
}
