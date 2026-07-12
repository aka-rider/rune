// Package workspace contains invariant checkers for the workspace page:
// SHADOW (buffer vs journal mirror), L1/L2 (layout bounds), EDITOR-TAB-COH
// (editor path matches active tab), TR-focus-valid, SAVE-SM, and the
// persistence/guard transition invariants (RESIZE-INV, SAVE-NOMUT, G1, G3,
// TR-cursor-not-dirty, RELOAD-NOMUT). DL1 (CheckDataLoss) and TR-dirty-clear
// (CheckSaveDirty) are store-derived and fed by the driver so this package stays
// docstate-free. The sole remaining L2 monitor (F1, quit liveness) lives in
// monitors.go; DL3 is subsumed by the store-derived SHADOW invariant, and
// EXT-NOCLOBBER is now an authoritative driver-level check (rs.externalWrites).
package workspace

import (
	"fmt"
	"reflect"
	"strings"

	"charm.land/lipgloss/v2"

	"rune/internal/fuzz/invariant"
	"rune/internal/fuzz/snapshot"
	"rune/pkg/ui/components/footer"
	"rune/pkg/ui/help"
)

// lipglossWidth wraps lipgloss.Width to keep the call sites readable.
func lipglossWidth(s string) int { return lipgloss.Width(s) }

func trunc(s string, n int) string { return invariant.Trunc(s, n) }

// pendingKind* mirror workspace.actionKind's iota order (None=0, Close=1,
// Quit=2, Evict=3, Trash=4), pinned by TestActionKindCasts in
// pkg/ui/pages/workspace/workspace_fuzz_test.go — Snapshot.PendingDataLossKind
// is that int cast, read here without importing the workspace package (this
// checker family stays decoupled from concrete production packages besides
// snapshot/footer/help — see the %T-name + reflect convention above).
const (
	pendingKindNone = iota
	pendingKindClose
	pendingKindQuit
	pendingKindEvict
	pendingKindTrash
)

// Check runs all L0 workspace invariants against s.
// Returns the first violation, or nil.
func Check(s snapshot.Snapshot) *invariant.Violation {
	// SHADOW: buffer content must match the independently-maintained journal mirror.
	if s.MirrorContent != "" && s.Content != s.MirrorContent {
		return &invariant.Violation{
			InvariantID: "SHADOW",
			Message: fmt.Sprintf("buffer %q != mirror %q",
				trunc(s.Content, 80), trunc(s.MirrorContent, 80)),
		}
	}

	// L1: every frame line's display width ≤ terminal width (no overflow).
	if s.Width > 0 && s.Frame != "" {
		for i, line := range strings.Split(s.Frame, "\n") {
			w := lipglossWidth(line)
			if w > s.Width {
				return &invariant.Violation{
					InvariantID: "L1",
					Message: fmt.Sprintf(
						"frame line %d display-width %d > terminal width %d", i, w, s.Width,
					),
				}
			}
		}
	}

	// L2: frame line count ≤ terminal height.
	if s.Height > 0 && s.Frame != "" {
		lines := strings.Count(s.Frame, "\n") + 1
		if lines > s.Height {
			return &invariant.Violation{
				InvariantID: "L2",
				Message: fmt.Sprintf(
					"frame has %d lines > terminal height %d", lines, s.Height,
				),
			}
		}
	}

	// EDITOR-TAB-COH: editor path equals the active tab's path. Exempt while a
	// load is in flight: close→neighbour and every async open leave the view as
	// the save-safe transitional untitled while the active tab already points at
	// the incoming doc (finalize, from pendingLoad). Coherence is restored when
	// the read settles; ⌘S is inert meanwhile, so the gap is safe (§1.4).
	// Equally exempt while a deferred reopen is pending (PendingReopenActive,
	// requestOpenPath's savingTarget gate): clicking back to a tab whose save
	// is still in flight moves the active tab immediately but deliberately
	// defers the view swap until the FileSavedMsg lands — the symmetric
	// window where the active handle leads the view, and equally save-safe
	// (startSave is inert while activeSave.InFlight).
	if !s.Loading && !s.PendingReopenActive && len(s.Tabs) > 0 && s.ActiveTabIdx >= 0 && s.ActiveTabIdx < len(s.Tabs) {
		activeTabPath := s.Tabs[s.ActiveTabIdx].Path
		if s.EditorPath != activeTabPath {
			return &invariant.Violation{
				InvariantID: "EDITOR-TAB-COH",
				Message: fmt.Sprintf(
					"EditorPath %q != Tabs[%d].Path %q",
					s.EditorPath, s.ActiveTabIdx, activeTabPath,
				),
			}
		}
	}

	// TR-focus-valid: FocusPane must be one of the known pane enum values (0–5).
	const maxPane = 5
	if s.FocusPane < 0 || s.FocusPane > maxPane {
		return &invariant.Violation{
			InvariantID: "TR-focus-valid",
			Message:     fmt.Sprintf("FocusPane %d not in [0, %d]", s.FocusPane, maxPane),
		}
	}

	// SAVE-SM: at most one in-flight save; SaveInFlight true requires SaveSnapshot non-nil.
	if s.SaveInFlight && s.SaveSnapshot == nil {
		return &invariant.Violation{
			InvariantID: "SAVE-SM",
			Message:     "save InFlight but SavedContent is nil (missing save identity)",
		}
	}

	// GUARD-STATE-COH: while a guard is showing, its kind must correlate with
	// a matching pending-state fact. ONE direction only (GuardVisible ⇒
	// correlate): footer clears its own guardKind/guardOptions synchronously
	// the instant the response keypress is processed — BEFORE the response
	// Cmd's DataLossGuardResponseMsg is delivered and the pending-state
	// handler clears its own flag one message later — so the reverse
	// direction (correlate ⇒ GuardVisible) false-positives during that gap.
	if s.GuardVisible {
		var correlated bool
		switch s.GuardKind {
		case footer.GuardMerge:
			correlated = s.PendingConflictActive
		case footer.GuardDeleted:
			correlated = s.PendingDeletedActive
		case footer.GuardRaced:
			correlated = s.PendingRacedActive
		case footer.GuardTrash:
			correlated = s.PendingDataLossKind == pendingKindTrash
		case footer.GuardDirty:
			correlated = s.PendingDataLossKind == pendingKindClose ||
				s.PendingDataLossKind == pendingKindQuit ||
				s.PendingDataLossKind == pendingKindEvict
		case footer.GuardDegraded:
			// GuardDegraded has no dedicated pendingXxx struct of its own
			// (startSaveDegradedConfirmed raises it straight from the
			// m.store.Degraded() check) — StoreDegraded (mirroring
			// m.footer.Degraded(), itself fixed from m.store.Degraded() at
			// store-open time) is the fact that must hold for it to have
			// been raised at all.
			correlated = s.StoreDegraded
		default:
			correlated = true // unknown/future kind — don't false-positive
		}
		if !correlated {
			return &invariant.Violation{
				InvariantID: "GUARD-STATE-COH",
				Message: fmt.Sprintf(
					"GuardKind=%v visible with no matching pending state (PendingConflictActive=%v PendingDeletedActive=%v PendingRacedActive=%v PendingDataLossKind=%d StoreDegraded=%v)",
					s.GuardKind, s.PendingConflictActive, s.PendingDeletedActive, s.PendingRacedActive, s.PendingDataLossKind, s.StoreDegraded,
				),
			}
		}
	}

	return nil
}

// CheckTransition runs workspace-domain L1 transition invariants.
// Returns all violations found.
func CheckTransition(prev snapshot.Snapshot, msg any, next snapshot.Snapshot) []invariant.Violation {
	var vs []invariant.Violation
	typeName := fmt.Sprintf("%T", msg)

	add := func(id, message string) {
		vs = append(vs, invariant.Violation{InvariantID: id, Message: message})
	}

	// RESIZE-INV: a WindowSizeMsg must not mutate buffer content, cursor positions, or dirty state.
	if typeName == "tea.WindowSizeMsg" {
		if next.Content != prev.Content {
			add("RESIZE-INV", fmt.Sprintf(
				"Content changed on resize: %q → %q",
				trunc(prev.Content, 40), trunc(next.Content, 40),
			))
		}
		if prev.HasDirtyFile != next.HasDirtyFile {
			add("RESIZE-INV", fmt.Sprintf(
				"HasDirtyFile changed on resize: %v → %v", prev.HasDirtyFile, next.HasDirtyFile,
			))
		}
	}

	// SAVE-NOMUT: a save message must not mutate the buffer content — EXCEPT
	// when the save is settling a Close/Quit/Evict guard response (kind != 0,
	// mirroring workspace.actionKind's None=0 sentinel). That save-then-close
	// flow legitimately swaps the displayed buffer to the next tab (or a blank
	// Untitled) once the save durably lands — pendingDataLoss is preserved
	// (not cleared) across the save specifically so FileSavedMsg's handler can
	// make that decision, so prev.PendingDataLossKind still reads the pending
	// kind at this transition. A PLAIN interactive ⌘S (kind==0) must still
	// never mutate the buffer it just saved.
	// (TR-dirty-clear moved to the driver-level, store-derived CheckSaveDirty,
	// keyed to the SAVED doc. The global next.HasDirtyFile used here fired a false
	// positive whenever any *other* tab was still dirty after saving one tab.)
	if typeName == "workspace.FileSavedMsg" && prev.PendingDataLossKind == 0 {
		if next.Content != prev.Content {
			add("SAVE-NOMUT", fmt.Sprintf(
				"Content changed during save: %q → %q",
				trunc(prev.Content, 40), trunc(next.Content, 40),
			))
		}
	}

	// G1: dirty file + ConfirmQuitMsg → guard must appear.
	if typeName == "footer.ConfirmQuitMsg" && prev.HasDirtyFile && !next.GuardVisible {
		add("G1", "dirty file + ConfirmQuitMsg did not raise guard")
	}

	// G3: dirty active tab + CloseFile key → guard must appear (unless guard already
	// active or a save is in-flight — in that case the key is consumed silently).
	if next.CloseFileKeyPressed && prev.ActiveTabDirty &&
		!prev.GuardVisible && !next.GuardVisible && !prev.SaveInFlight {
		add("G3", "dirty active tab + CloseFile key (^w) did not raise guard")
	}

	// TR-cursor-not-dirty: an EDITOR-directed key press that does not change the
	// editor buffer must not set the dirty flag. Gated to the editor pane
	// (FocusPane==paneEditor): a keystroke routed to the title or chat surface
	// legitimately journals an edit there — marking the doc dirty — WITHOUT changing
	// the editor content, so firing on those is a false positive (the predicate
	// only compares editor Content). The invariant's intent is "editor navigation
	// does not dirty"; that only holds when the editor is the key target.
	const paneEditor = 2 // FocusPane: 0=tree 1=tabs 2=center(editor) 3=title 4=chat 5=search
	if typeName == "tea.KeyPressMsg" && prev.FocusPane == paneEditor &&
		!prev.HasDirtyFile && next.HasDirtyFile && next.Content == prev.Content {
		add("TR-cursor-not-dirty", "editor-focused key press set dirty flag without any content change")
	}

	// RELOAD-NOMUT: a dirChangedMsg-driven reload must not mutate buffer content,
	// cursor state, or dirty status — only the filetree listing changes.
	if typeName == "workspace.dirChangedMsg" || typeName == "filetree.DirReloadedMsg" {
		if next.Content != prev.Content {
			add("RELOAD-NOMUT", fmt.Sprintf(
				"Content changed on dir reload: %q → %q",
				trunc(prev.Content, 40), trunc(next.Content, 40),
			))
		}
		if prev.HasDirtyFile != next.HasDirtyFile {
			add("RELOAD-NOMUT", fmt.Sprintf(
				"HasDirtyFile changed on dir reload: %v → %v",
				prev.HasDirtyFile, next.HasDirtyFile,
			))
		}
	}

	// TRASH-DIRTY-BLOCK / TRASH-OPT-REMOVE / TRASH-GUARD-RAISED: FileDeleteRequestedMsg invariants.
	// When the targeted path is the dirty active document, RemoveEntry must NOT run
	// (§1.4.4 guard bailed first) → FiletreeLen unchanged.
	// When it is not the dirty active document, a confirmation guard must appear and
	// the filetree must remain unchanged (removal deferred until guard confirmation).
	if typeName == "filetree.FileDeleteRequestedMsg" {
		path := reflect.ValueOf(msg).FieldByName("Path").String()
		isDirtyActive := path == prev.ActiveFilePath && prev.ActiveTabDirty
		if isDirtyActive {
			if next.FiletreeLen < prev.FiletreeLen {
				add("TRASH-DIRTY-BLOCK", fmt.Sprintf(
					"FileDeleteRequestedMsg for dirty active %q removed entry (len %d→%d); §1.4.4 guard bypassed",
					path, prev.FiletreeLen, next.FiletreeLen))
			}
		} else {
			if next.FiletreeLen != prev.FiletreeLen {
				add("TRASH-OPT-REMOVE", fmt.Sprintf(
					"FileDeleteRequestedMsg for %q mutated filetree before guard confirmation (len %d→%d)",
					path, prev.FiletreeLen, next.FiletreeLen))
			}
			if !next.GuardVisible {
				add("TRASH-GUARD-RAISED", fmt.Sprintf(
					"FileDeleteRequestedMsg for %q did not raise confirmation guard", path))
			}
		}
	}

	// TRASH-TAB-GONE: after FileDeletedMsg the deleted path must not remain in
	// the tab bar — either the active tab closed (executeClose) or the background
	// tab was removed (opentabs.CloseFile).
	if typeName == "workspace.FileDeletedMsg" {
		path := reflect.ValueOf(msg).FieldByName("Path").String()
		for _, tab := range next.Tabs {
			if tab.Path == path {
				add("TRASH-TAB-GONE", fmt.Sprintf(
					"FileDeletedMsg{Path:%q} but tab still present in next snapshot", path))
				break
			}
		}
	}

	// GUARD-RESPONSE-CLEARS: after any footer.DataLossGuardResponseMsg,
	// PendingConflictActive must not be left as a LINGERING stale flag —
	// either it cleared, or a FRESH conflict was legitimately re-raised
	// (visibly correlated: GuardVisible && GuardKind==GuardMerge, e.g.
	// startSave's own §1.4.8 pre-write re-check re-detecting SyncDiverged
	// from inside THIS same response's handler). A lingering pendingConflict
	// with no correlated guard showing would misroute a LATER, unrelated
	// dirty-guard [D]iscard into handleDataLossDiscardConflict ("load
	// theirs") instead of the dirty-close discard the user actually pressed.
	if typeName == "footer.DataLossGuardResponseMsg" && next.PendingConflictActive &&
		!(next.GuardVisible && next.GuardKind == footer.GuardMerge) {
		add("GUARD-RESPONSE-CLEARS", "PendingConflictActive still true after DataLossGuardResponseMsg with no correlated GuardMerge visible")
	}

	// MERGE-GUARD-RAISE / DELETED-GUARD-RAISE / QUIT-ABORT: workspace.FileSaveErrorMsg routing.
	if typeName == "workspace.FileSaveErrorMsg" {
		rv := reflect.ValueOf(msg)
		docID := rv.FieldByName("DocID").Int()
		requestID := rv.FieldByName("RequestID").String()
		conflict := rv.FieldByName("Conflict").Bool()
		missing := rv.FieldByName("Missing").Bool()

		// Gate reproduces handleFileSaveErrorMsg's own routing condition
		// EXACTLY: m.activeSave.InFlight && m.activeSave.RequestID==msg.RequestID
		// && msg.DocID==m.view.DocID() && m.view.IsFile(). The RequestID
		// correlation (critic R1) excludes a quit-batch/evict background
		// save's ack for the SAME doc from being misread as THIS interactive
		// save's conflict, even when both are concurrently in flight.
		// ActiveFilePath!="" && !=help.DocPath approximates IsFile() (help's
		// Path() is also non-empty, §1.7 — no magic-string proxy without the
		// exclusion).
		correlated := prev.SaveInFlight && requestID == prev.SaveRequestID &&
			docID == prev.DocID && prev.ActiveFilePath != "" && prev.ActiveFilePath != help.DocPath

		if correlated && conflict {
			if !(next.GuardVisible && next.GuardKind == footer.GuardMerge && next.PendingConflictActive) {
				add("MERGE-GUARD-RAISE", fmt.Sprintf(
					"FileSaveErrorMsg{Conflict:true} for correlated interactive save (doc %d) did not raise GuardMerge (GuardVisible=%v GuardKind=%v PendingConflictActive=%v)",
					docID, next.GuardVisible, next.GuardKind, next.PendingConflictActive))
			}
		}
		if correlated && missing {
			if !(next.GuardVisible && next.GuardKind == footer.GuardDeleted && next.PendingDeletedActive) {
				add("DELETED-GUARD-RAISE", fmt.Sprintf(
					"FileSaveErrorMsg{Missing:true} for correlated interactive save (doc %d) did not raise GuardDeleted (GuardVisible=%v GuardKind=%v PendingDeletedActive=%v)",
					docID, next.GuardVisible, next.GuardKind, next.PendingDeletedActive))
			}
		}

		// QUIT-ABORT: a FileSaveErrorMsg landing while a quit-batch save was
		// pending aborts the WHOLE quit (handleFileSaveErrorMsg's
		// actionQuit branch) — pending must clear and quit must not proceed.
		if prev.PendingDataLossKind == pendingKindQuit {
			if next.PendingDataLossKind == pendingKindQuit {
				add("QUIT-ABORT", "PendingDataLossKind still Quit after FileSaveErrorMsg during quit batch")
			}
			if next.AppQuitting {
				add("QUIT-ABORT", "AppQuitting true after FileSaveErrorMsg aborted the quit batch")
			}
		}
	}

	return vs
}

// CheckDataLoss checks DL1: VFS content must equal buffer content immediately
// after an autosave snapshot settles.
// vfsContent is the result of store.Content(snap.DocID), passed by the driver
// so this package remains docstate-free (N2).
func CheckDataLoss(s snapshot.Snapshot, vfsContent string) *invariant.Violation {
	if s.DocID == 0 {
		return nil // no VFS doc yet — untitled without a scratch allocation
	}
	// An empty document is valid: when the user deletes all text, autosave snapshots
	// "" and RecoverDocument reconstructs "" faithfully. The sole data-loss signal is
	// a genuine mismatch between durable VFS content and the live buffer — an
	// unconditional vfsContent=="" guard fired on legitimately-empty documents. (The
	// "no VFS record at all" case is distinguished upstream by HasHistory, and the
	// driver skips this check entirely on a store read error.)
	if vfsContent != s.Content {
		return &invariant.Violation{
			InvariantID: "DL1",
			Message: fmt.Sprintf(
				"VFS[:%d]=%q != buffer[:%d]=%q",
				min(len(vfsContent), 40), trunc(vfsContent, 40),
				min(len(s.Content), 40), trunc(s.Content, 40),
			),
		}
	}
	return nil
}

// CheckSaveDirty checks TR-dirty-clear: after a FileSavedMsg settles, the SAVED
// document must be clean. savedDocDirty is the store's derived dirty state for the
// saved doc (a live event between saved_seq and current_seq), passed by the driver
// so this package stays docstate-free (N2). Keying to the saved doc — not the global
// any-tab HasDirtyFile flag — means a still-dirty *other* tab no longer trips a false
// positive. A residual dirty here means saved_seq did not advance to cover the bytes
// that were written (the §1.4.2/§1.4.8 MarkSaved hazard).
func CheckSaveDirty(savedDocDirty bool) *invariant.Violation {
	if savedDocDirty {
		return &invariant.Violation{
			InvariantID: "TR-dirty-clear",
			Message:     "saved document still dirty after FileSavedMsg settled (saved_seq did not advance to cover the buffer)",
		}
	}
	return nil
}
