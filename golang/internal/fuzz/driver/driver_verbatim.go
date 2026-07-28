package driver

import (
	"fmt"
	"reflect"
	"strings"

	tea "charm.land/bubbletea/v2"

	"rune/internal/fuzz/invariant"
	"rune/internal/fuzz/snapshot"
	"rune/pkg/dictation"
	"rune/pkg/docstate"
	"rune/pkg/ui/components/footer"
	pgworkspace "rune/pkg/ui/pages/workspace"
)

// checkVerbatim runs the WP2 driver-level ground-truth checks: the
// disk↔buffer verbatim edge of §1.4.5 that nothing before WP2 compared
// (DL1/WP7(b) only pin store↔buffer; capture-before-discard only pins
// blob-existence). Extracted from drainMsg to keep driver.go under the
// 500-LoC limit (§1.6/§11) — called inline from drainMsg with the exact
// same rs/m/msg/prev/snap it already has in scope.
func checkVerbatim(rs *runState, m pgworkspace.Model, msg tea.Msg, prev, snap snapshot.Snapshot) *invariant.Violation {
	if v := checkSaveVerbatim(rs, msg); v != nil {
		return v
	}
	if v := checkLoadVerbatim(rs, msg, snap); v != nil {
		return v
	}
	if v := checkCloseNoLoss(rs, msg, prev, snap); v != nil {
		return v
	}
	if v := checkRedoClear(rs, m, msg, prev, snap); v != nil {
		return v
	}
	if v := checkDictNoDestroy(msg, prev, snap); v != nil {
		return v
	}
	return nil
}

// DICT-NO-DESTROY (§1.3 rung-1): a dictation-engine message must never
// destructively clear a non-empty buffer. Two forms:
//  1. PartialTranscriptionMsg with a whitespace-only Accumulated is an
//     upstream reset (dictation.go's own TrimSpace guard) — the buffer must
//     be byte-identical before and after.
//  2. Backstop, independent of msg shape: NO dictation-family message may
//     take a non-empty buffer to empty. This is what the FinalTranscriptionMsg
//     empty-final hazard (fixed in pkg/ui/components/dictation/dictation.go —
//     the Partial handler had a TrimSpace guard the Final handler lacked)
//     would have violated; kept as a backstop so any future dictation
//     handler regresses loudly instead of silently.
func checkDictNoDestroy(msg tea.Msg, prev, next snapshot.Snapshot) *invariant.Violation {
	if partial, ok := msg.(dictation.PartialTranscriptionMsg); ok {
		if strings.TrimSpace(partial.Accumulated) == "" && next.Content != prev.Content {
			return &invariant.Violation{
				InvariantID: "DICT-NO-DESTROY",
				Message: fmt.Sprintf("whitespace-only PartialTranscriptionMsg{Accumulated=%q} changed the buffer: %q -> %q",
					partial.Accumulated, invariant.Trunc(prev.Content, 40), invariant.Trunc(next.Content, 40)),
			}
		}
	}
	isDictMsg := false
	switch msg.(type) {
	case dictation.PartialTranscriptionMsg, dictation.FinalTranscriptionMsg, dictation.ErrorMsg:
		isDictMsg = true
	}
	if isDictMsg && prev.Content != "" && next.Content == "" {
		return &invariant.Violation{
			InvariantID: "DICT-NO-DESTROY",
			Message:     fmt.Sprintf("%T emptied a non-empty buffer (was %q)", msg, invariant.Trunc(prev.Content, 40)),
		}
	}
	return nil
}

// SAVE-VERBATIM: after a COMMITTED FileSavedMsg, the bytes actually on the
// (shared) VFS must byte-equal what the journal now holds for that doc —
// closing the last edge of the buffer↔journal↔disk triangle DL1/WP7(b)
// leave unchecked (they only ever compare buffer↔journal). Skipped under
// RunReorderSaves (msg delivery is deliberately deferred/reordered there —
// SAVE-RACE owns that durability property) and on any store/mem read error
// (quit teardown closes the store mid-run — same discipline as DL1).
func checkSaveVerbatim(rs *runState, msg tea.Msg) *invariant.Violation {
	if rs.reorderSaves || rs.mem == nil || rs.store == nil {
		return nil
	}
	saved, ok := msg.(pgworkspace.FileSavedMsg)
	if !ok || !saved.Result.Committed {
		return nil
	}
	diskBytes, err := rs.mem.ReadFile(saved.Path)
	if err != nil {
		return nil // fire-and-forget: transient/torn-down VFS, not a data-integrity signal
	}
	journalContent, err := rs.store.Content(saved.DocID)
	if err != nil {
		return nil
	}
	if string(diskBytes) != journalContent {
		return &invariant.Violation{
			InvariantID: "SAVE-VERBATIM",
			Message: fmt.Sprintf("after FileSavedMsg{Committed} for %s: mem bytes[:40]=%q != journal content[:40]=%q",
				invariant.Trunc(saved.Path, 60), invariant.Trunc(string(diskBytes), 40), invariant.Trunc(journalContent, 40)),
		}
	}
	return nil
}

// LOAD-VERBATIM: after an APPLIED FileLoadedMsg — the load actually landed
// (!next.Loading), for the doc/path it's still relevant to
// (next.ActiveFilePath==msg.Path — a superseded/stale load is excluded by
// definition), no guard raised (a raced/conflicted load legitimately shows
// something other than a byte-for-byte disk mirror), and Load's OWN
// SyncState reports SyncClean — the displayed buffer must byte-equal what's
// actually on the (shared) VFS. Catches a CRLF/newline/BOM normalization
// slipping in on load (§1.4.5).
//
// Gated on loaded.Result.Sync.Kind == docstate.SyncClean, NOT on
// rs.store.CurrentSeq(docID) being 0 / no active edits since this load (the
// original WP2 gate): the two are NOT equivalent. Active edits since THIS
// load are always zero for a just-applied load, regardless of history. But Load
// returns Recovered (the journal-reconstructed content, §1.4.3) which
// legitimately differs from DiskContent whenever the journal's current
// position is behind what's on disk — e.g. type → save (writes disk) → undo
// (journal head moves back, buffer reverts) → later reload: Recovered
// correctly reconstructs the PRE-save state, disk still holds the SAVED
// bytes. Both are correct; they're just not equal. Sync.Kind (BufferAhead/
// DiskAhead/Diverged vs Clean) is what Load itself already computed to
// classify exactly this, and is authoritative — found via
// FuzzHumanSession's unicodeTyping+save+undo+Help-reopen sequence
// false-positiving on a fully-correct recovery reconstruction.
func checkLoadVerbatim(rs *runState, msg tea.Msg, snap snapshot.Snapshot) *invariant.Violation {
	if rs.mem == nil || rs.store == nil {
		return nil
	}
	loaded, ok := msg.(pgworkspace.FileLoadedMsg)
	if !ok {
		return nil
	}
	if snap.Loading || snap.ActiveFilePath != loaded.Path || snap.DocID == 0 || snap.GuardVisible {
		return nil
	}
	if loaded.Result.Sync.Kind != docstate.SyncClean {
		return nil // buffer legitimately diverges from disk — not a verbatim violation
	}
	diskBytes, err := rs.mem.ReadFile(loaded.Path)
	if err != nil {
		return nil
	}
	if string(diskBytes) != snap.Content {
		return &invariant.Violation{
			InvariantID: "LOAD-VERBATIM",
			Message: fmt.Sprintf("after applied FileLoadedMsg for %s: buffer[:40]=%q != mem bytes[:40]=%q",
				invariant.Trunc(loaded.Path, 60), invariant.Trunc(snap.Content, 40), invariant.Trunc(string(diskBytes), 40)),
		}
	}
	return nil
}

// CLOSE-NO-LOSS: when a bound tab (DocID≠0, Path≠"") disappears from the tab
// set, the doc's journal history must survive the close (HasHistory) —
// EXCEPT two explicit-user-confirmed purge paths (§1.4.4: the confirmation
// IS what makes discarding history correct, not a bug): a real trash
// confirm (FileDeletedMsg) and GuardDeleted's own [D]iscard response
// (handleDeletedDiscard calls store.DeleteDoc directly — "the user's
// explicit, prompt-confirmed choice to purge the doc's VFS history", its own
// doc comment) — detected via prev.PendingDeletedActive so this doesn't also
// swallow an ordinary dirty-close/evict Discard, which never purges
// history. Diffing BY DocID (not path) makes this rename-proof. Covers
// ^w-discard, clean evict, guarded evict, and evict-save-ack close in one
// place.
//
// Content equality against a last-observed-content cache was PART of this
// check (WP2) but is deliberately removed (WP7 soak finding): any such cache
// is sampled, not exhaustive — it was only refreshed every ~8th step, or on
// load/save transitions, for whatever doc happened to be ACTIVE at sample
// time — and was NEVER refreshed for a doc that received further LEGITIMATE
// edits while it sat in the background. A closing tab whose content had
// simply advanced past a stale sample (more typing, not data loss)
// false-positived — reproduced and confirmed via a 30-minute soak. The cache
// itself is gone (this sub-check was its last consumer); HasHistory alone is
// the check's load-bearing, always-sound half.
func checkCloseNoLoss(rs *runState, msg tea.Msg, prev, snap snapshot.Snapshot) *invariant.Violation {
	if rs.store == nil {
		return nil
	}
	if _, isDelete := msg.(pgworkspace.FileDeletedMsg); isDelete {
		return nil
	}
	if resp, ok := msg.(footer.DataLossGuardResponseMsg); ok &&
		resp.Response == footer.DataLossDiscard && prev.PendingDeletedActive {
		return nil
	}
	present := make(map[int64]bool, len(snap.Tabs))
	for _, t := range snap.Tabs {
		if t.DocID != 0 {
			present[t.DocID] = true
		}
	}
	for _, t := range prev.Tabs {
		if t.DocID == 0 || t.Path == "" || present[t.DocID] {
			continue
		}
		// t departed this step and wasn't a trash-confirmed removal.
		hasHistory, err := rs.store.HasHistory(t.DocID)
		if err != nil {
			continue // fire-and-forget: store read error, not a data-integrity signal
		}
		if !hasHistory {
			// A genuinely never-edited doc legitimately has zero history: naming
			// (title-only, never journaled — workspace_journal.go's target=="title"
			// guard) and bind-new-materializing a still-empty untitled doc writes a
			// real, empty file and rebinds its docID with NOTHING ever appended to
			// events/snapshots — there was nothing to lose. Distinguish that from an
			// actual loss by checking the file's own bytes: non-empty disk content
			// with zero history is the genuinely inconsistent case worth flagging;
			// an empty (or absent/unreadable) file alongside zero history is not.
			if rs.mem != nil {
				if diskBytes, readErr := rs.mem.ReadFile(t.Path); readErr != nil || len(diskBytes) == 0 {
					continue
				}
			}
			return &invariant.Violation{
				InvariantID: "CLOSE-NO-LOSS",
				Message: fmt.Sprintf("tab for doc %d (%s) closed via %T but store.HasHistory now false",
					t.DocID, invariant.Trunc(t.Path, 60), msg),
			}
		}
	}
	return nil
}

// REDO-CLEAR: after a buffer-changing KeyPressMsg (excluding Undo/Redo
// themselves — they MOVE the journal position, they don't abandon a redo
// future) or PasteMsg on the same doc, store.RedoPeek(docID) must report
// ok==false: AppendEdit truncates the abandoned future on every fresh edit
// (pkg/docstate/journal.go), so a live redo entry surviving past a NEW edit
// is a zombie that would resurrect abandoned bytes. "Buffer-changing" is
// approximated by Content actually differing across the transition — a
// navigation-only key press that leaves Content unchanged never appends
// anything for AppendEdit to truncate against.
//
// Gated on snap.DocID == prev.DocID: a DOCUMENT-SWITCHING key (F1/Help
// toggle back to the previously active doc, tab switch, etc.) also changes
// Content — because it's now displaying a DIFFERENT buffer — with no edit
// to EITHER doc involved. Without this, returning from Help to a doc with a
// live redo (2 undos, never redone) false-positived: Content genuinely
// differs (Help's text vs the doc's), but nothing was ever appended to that
// doc's journal in this transition (found via FuzzHumanSession's F1/Help
// cluster on a doc with a pending redo, WP7 session).
func checkRedoClear(rs *runState, m pgworkspace.Model, msg tea.Msg, prev, snap snapshot.Snapshot) *invariant.Violation {
	if rs.store == nil || snap.DocID == 0 || snap.DocID != prev.DocID || snap.Content == prev.Content {
		return nil
	}
	switch msg.(type) {
	case tea.KeyPressMsg:
		if m.IsUndoRedoMsg(msg) {
			return nil
		}
	case tea.PasteMsg:
		// buffer-changing paste — falls through to the check below.
	default:
		return nil
	}
	_, ok, err := rs.store.RedoPeek(snap.DocID)
	if err != nil {
		return nil // fire-and-forget: corrupt/unreadable journal is surfaced elsewhere, not here
	}
	if ok {
		return &invariant.Violation{
			InvariantID: "REDO-CLEAR",
			Message: fmt.Sprintf("doc %d: a live redo entry survived a buffer-changing %T — AppendEdit should have truncated the abandoned future",
				snap.DocID, msg),
		}
	}
	return nil
}

// checkMergeLifecycle runs the WP5 merge-lifecycle checks (DISCARD-ADOPT,
// MERGE-RESOLVE-ADOPT, UNWIND-REDETECT(L1)) — reachable reliably only once
// WP4's mergeResolve cluster lands. workspace.resolveProbeMsg/unwindProbeMsg
// are unexported (package workspace, not pgworkspace-importable), so this
// reads them via the %T-name + reflect convention the leaf checker packages
// already use for unexported msg fields — reading an unexported field's
// primitive value via reflect (Bool/Int/String/IsNil) is permitted; only
// .Interface() on it is not.
func checkMergeLifecycle(rs *runState, msg tea.Msg, next snapshot.Snapshot) *invariant.Violation {
	if rs.store == nil || rs.reorder {
		return nil
	}
	typeName := fmt.Sprintf("%T", msg)
	switch typeName {
	case "workspace.resolveProbeMsg":
		return checkResolveProbe(rs, msg, next)
	case "workspace.unwindProbeMsg":
		return checkUnwindProbe(msg, next)
	}
	return nil
}

// mergeIntentMerge/mergeIntentDiscard mirror workspace.mergeIntent's iota
// order (workspace_merge_fresh.go: mergeIntentMerge=0, mergeIntentDiscard=1).
const (
	mergeIntentMerge   = 0
	mergeIntentDiscard = 1
)

func checkResolveProbe(rs *runState, msg tea.Msg, next snapshot.Snapshot) *invariant.Violation {
	rv := reflect.ValueOf(msg)
	if rv.FieldByName("err").IsNil() == false || rv.FieldByName("missing").Bool() {
		return nil // invalid probe (error/missing) — routes to GuardDeleted, not a resolve
	}
	docID := rv.FieldByName("ticket").FieldByName("docID").Int()
	path := rv.FieldByName("path").String()
	intent := rv.FieldByName("intent").Int()

	// A STALE ticket (the view moved on — a later transition changed the
	// displayed doc, or forced an unconditional epoch bump — Part IV,
	// driver_delayed_view.go's VIEW-TICKET-STALE scenario) is refused
	// outright by applyViewResult: neither applyDiscardConflict nor
	// applyMergeConflict/resolveAdoptAt ever ran. next.DocID no longer
	// matching this message's own ticket docID is exactly that signal —
	// asserting a post-resolve state for a resolve that never applied would
	// false-positive (empirically confirmed via FuzzDelayedViewResult's own
	// seed corpus).
	if next.DocID != docID {
		return nil
	}

	switch intent {
	case mergeIntentDiscard:
		// DISCARD-ADOPT: a valid discard resolves the doc to Clean, not
		// dirty, with the buffer matching what's actually on the (shared)
		// VFS — applyDiscardConflict installs theirs verbatim and adopts
		// the CAS baseline to it (resolveAdoptAt) in the SAME journaled
		// transition.
		sync, err := rs.store.Sync(docID)
		if err != nil {
			return nil
		}
		if sync.Kind != docstate.SyncClean {
			return &invariant.Violation{
				InvariantID: "DISCARD-ADOPT",
				Message:     fmt.Sprintf("doc %d: Sync.Kind=%v after a valid discard resolve, want SyncClean", docID, sync.Kind),
			}
		}
		if dirty, err := rs.store.IsDirty(docID); err == nil && dirty {
			return &invariant.Violation{
				InvariantID: "DISCARD-ADOPT",
				Message:     fmt.Sprintf("doc %d: still dirty after a valid discard resolve", docID),
			}
		}
		if rs.mem != nil {
			if diskBytes, err := rs.mem.ReadFile(path); err == nil && string(diskBytes) != next.Content {
				return &invariant.Violation{
					InvariantID: "DISCARD-ADOPT",
					Message: fmt.Sprintf("doc %d: buffer[:40]=%q != mem bytes[:40]=%q after a valid discard resolve",
						docID, invariant.Trunc(next.Content, 40), invariant.Trunc(string(diskBytes), 40)),
				}
			}
		}
	default: // mergeIntentMerge
		// MERGE-RESOLVE-ADOPT: applyMergeConflict's resolveAdoptAt call runs
		// SYNCHRONOUSLY and UNCONDITIONALLY (not gated on whether conflict
		// blocks remain unresolved) — it re-stamps docID's CAS baseline
		// (saved_obs) to the FRESH theirs observation this resolve was
		// based on, "so a resolved-merge ⌘S sees a clean expect and writes
		// cleanly" (its own doc comment). That is the one, precise, checkable
		// claim "adopted" makes.
		//
		// PLAN DEVIATION (verified by reading classifySync/Sync/ResolveAdopt,
		// not guessed): the plan's original phrasing ("Sync(doc).Kind !=
		// SyncDiverged") does NOT hold for a genuine merge that combines
		// content from BOTH sides (the normal, interesting case) — only for
		// the degenerate case where the result exactly equals one side.
		// classifySync's ancestor is the ORIGINAL pre-conflict ancestor (an
		// older 'load'/'save'/'resolve' observation — ancestorAt excludes
		// the just-inserted 'resolve' row via its self-reference guard), and
		// theirs is that SAME 'resolve' row (blob_hash = original theirs,
		// NOT the merged buffer). A combined ours+theirs buffer then differs
		// from BOTH ancestor and theirs, landing classifySync's "Otherwise:
		// Diverged — both sides moved independently" branch — CORRECTLY:
		// the merged buffer really is a third version, distinct from either
		// side, and legitimately stays classified that way until an
		// explicit ⌘S journals a fresh 'save' observation matching it.
		// Empirically confirmed via two independent FuzzHumanSession finds
		// (seed#22, a one-block conflict; 47934de71a3dbbf6, a genuine
		// zero-conflict auto-merge) — both are CORRECT production behavior,
		// not partial resolution.
		saved, ok, err := rs.store.SavedObs(docID)
		if err != nil || !ok {
			return nil
		}
		theirsHash := rv.FieldByName("state").FieldByName("Theirs").FieldByName("Hash").String()
		if saved.BlobHash != theirsHash {
			return &invariant.Violation{
				InvariantID: "MERGE-RESOLVE-ADOPT",
				Message: fmt.Sprintf("doc %d: CAS baseline (saved_obs hash %s) was not re-stamped to the fresh theirs (%s) this resolve was based on",
					docID, invariant.Trunc(saved.BlobHash, 12), invariant.Trunc(theirsHash, 12)),
			}
		}
	}
	return nil
}

func checkUnwindProbe(msg tea.Msg, next snapshot.Snapshot) *invariant.Violation {
	rv := reflect.ValueOf(msg)
	if rv.FieldByName("err").IsNil() == false || rv.FieldByName("missing").Bool() {
		return nil // invalid probe — handleUnwindProbe degrades safely or raises GuardDeleted instead
	}
	docID := rv.FieldByName("ticket").FieldByName("docID").Int()
	// A stale ticket (view moved on since the probe was issued) is refused
	// by applyViewResult — see checkResolveProbe's identical gate.
	if next.DocID != docID {
		return nil
	}
	if next.MergeActive {
		return nil // a redo re-entered the resolver before this probe landed
	}
	kind := rv.FieldByName("state").FieldByName("Kind").Int()
	if kind == int64(docstate.SyncClean) {
		return nil // nothing left to reconcile
	}
	if !(next.GuardVisible && next.GuardKind == footer.GuardMerge && next.PendingConflictActive) {
		return &invariant.Violation{
			InvariantID: "UNWIND-REDETECT",
			Message:     fmt.Sprintf("doc %d: unwindProbeMsg (Kind=%d, non-clean) did not re-raise GuardMerge", docID, kind),
		}
	}
	return nil
}
