package workspace

import (
	"fmt"
	"path/filepath"
	"strings"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/ui/components/footer"
	"rune/pkg/ui/components/opentabs"
	"rune/pkg/ui/pages/workspace/mergemode"
)

// handleFileSavedMsg processes a FileSavedMsg. Returns done=true when the
// multi-tab quit save batch completes and the caller should return immediately
// (the returned cmds include the teardown/quit sequence). Extracted from Update
// to keep workspace_update.go under the 500-LoC limit (§1.6/§11).
//
// WP5: shrinks to UI-only bookkeeping — store.Materialize's own commit tx
// already did everything durable (observation + saved_obs + re-Bind, S5:
// closed by construction, no fire-and-forget acks left to run here).
func (m Model) handleFileSavedMsg(msg FileSavedMsg, cmds []tea.Cmd) (Model, []tea.Cmd, bool) {
	// A successful materialize resolves any prior sticky top-banner error
	// (focus-trap fix, §ACTIVE(2)) — disk I/O just succeeded.
	m.err = nil

	// Interactive ⌘S, close-save, or bind-new (tracked by activeSave).
	if m.activeSave.InFlight && m.activeSave.RequestID == msg.RequestID {
		m.activeSave.InFlight = false
		// stillDisplayed reports whether the file THIS save just wrote is still
		// the one shown in m.view. Navigating AWAY from a saving file has no
		// InFlight gate (only navigating BACK into it does — requestOpenPath's
		// savingTarget guard) — so by the time this ack lands, m.view may already
		// be a different, unrelated document. Every store/view mutation below
		// keys off msg.DocID (the save's own captured identity), never m.view's
		// current one — otherwise this ack corrupts whatever document the user
		// has since switched to. docID alone is the right comparison (not a
		// path-sensitive TabHandle): docID is stable across ALL three save
		// shapes this ack can represent — an ordinary overwrite, a true
		// bind-new (view is still untitled, path=="" until bindMaterialized
		// below adopts msg.Path), and a bindNew=true recreate-after-delete
		// (view was ALREADY a file at msg.Path the whole time) — a
		// path-sensitive comparison would wrongly read the third case as "no
		// longer displayed" (§1.4.6: identity is the doc, not its path).
		stillDisplayed := msg.DocID != 0 && msg.DocID == m.view.DocID()
		if stillDisplayed {
			// The merge result has been persisted — leave the resolver (BUG3). A clean
			// or fully-resolved merge is the only way to reach a successful save while
			// merge mode lingers; Reset is a no-op otherwise.
			m.merge = mergemode.Reset(m.merge)
			// Fix C (BUG1): a successful save reconciles whatever external change
			// the persistent "changed on disk" indicator was warning about.
			m = m.setDiskChangedHint(false)
		}
		// WP-R4 item 6: saving through a hardlinked path forks the document
		// from its other names on disk (the atomic write breaks the link).
		// D13: Saved.NLink is sql.NullInt64 (§1.7) — Valid=false means the
		// post-write stat couldn't determine a link count, which is neither
		// "hardlinked" nor "not hardlinked"; the warning only fires when we
		// actually know it's >1.
		if msg.Result.Committed && msg.Result.Saved.NLink.Valid && msg.Result.Saved.NLink.Int64 > 1 {
			var linkCmd tea.Cmd
			m.footer, linkCmd = m.footer.Update(footer.ShowStatusMsg{Text: "⚠ hardlinked file — saving breaks the link"})
			cmds = append(cmds, linkCmd)
		}
		if msg.Result.Raced {
			// F5: our write committed for real (Materialize's own commitSave
			// already ran the moment the swap-race was detected), but a
			// concurrent writer's bytes were displaced and captured. This is
			// a DISTINCT resolution path (critic R1) — mark clean (the save
			// genuinely committed) and raise the Raced guard instead of the
			// ordinary bindNew/close bookkeeping below, which assumes no
			// further decision is pending.
			m.opentabs = m.opentabs.SetDirty(opentabs.TabHandle{DocID: msg.DocID, Path: msg.Path}, false)
			// The raced guard supersedes any pending destructive continuation:
			// a close-save that raced must NOT leave guard.cont dangling — a
			// LATER unrelated save ack would match the "stillDisplayed &&
			// cont.owns(contClose, ...)" bookkeeping below and close the tab
			// out from under the user, minutes after they chose [K]eep-mine
			// and kept working (review finding). The close continuation is
			// cancelled; the user re-issues ^W once the race is resolved.
			// Gated on cont.owns(contClose, ...) (GUARD-STATE-COH): only
			// clear/dismiss when THIS save is the one guard.cont is waiting
			// on — an unrelated save (bind-new, restore-theirs) racing must
			// never touch a guard it has no idea exists.
			if m.guard.cont.owns(contClose, msg.RequestID) {
				m.guard.cont = continuation{}
				m = m.clearGuardPrompt()
			}
			// surfaceRaced (W6): raise when still displayed, else queue for
			// load-settle — a race against a concurrent writer must never
			// resolve silently (review finding: the displaced bytes were
			// otherwise reachable only by blob archaeology).
			m = m.surfaceRaced(stillDisplayed, msg.DocID, msg.Path, msg.Result.Saved, msg.Result.Fresh, &cmds)
			var reopenCmd tea.Cmd
			m, reopenCmd = m.flushPendingReopen()
			cmds = append(cmds, reopenCmd)
			return m, cmds, false
		}
		if msg.BindNew {
			m = m.bindMaterialized(msg.DocID, msg.Path, stillDisplayed)
			if root := m.filetree.Root(); root != "" {
				cmds = append(cmds, reloadDirCmd(m.fsys(), root))
			}
		}
		m.opentabs = m.opentabs.SetDirty(opentabs.TabHandle{DocID: msg.DocID, Path: msg.Path}, false)
		// Gated on cont.owns(contClose, ...), not just guard.cont.kind==contClose
		// (GUARD-STATE-COH): an unrelated save (e.g. a bind-new triggered by
		// finalizing the title in the same keypress that also raised a close
		// guard for a completely different reason) must never execute someone
		// else's still-pending close decision just because guard.cont happens
		// to coincidentally hold contClose right now.
		if stillDisplayed && m.guard.cont.owns(contClose, msg.RequestID) {
			m.guard.cont = continuation{}
			m = m.clearGuardPrompt()
			var closeCmd tea.Cmd
			m, closeCmd = m.executeClose(m.view.DocID(), m.view.Path())
			cmds = append(cmds, closeCmd)
		}
		var reopenCmd tea.Cmd
		m, reopenCmd = m.flushPendingReopen()
		cmds = append(cmds, reopenCmd)
		return m, cmds, false
	}
	// Eviction background save ack: victim is clean, close it, open pending file.
	if m.guard.cont.owns(contEvict, msg.RequestID) {
		if msg.Result.Raced {
			// A swap-race during an LRU evict save must never close the
			// victim tab and move on as if nothing happened (review finding:
			// the concurrent writer's displaced bytes would be reachable
			// only by blob archaeology). The save itself committed — mark
			// clean — but the eviction is ABORTED: the victim tab stays, the
			// pending open is dropped (surfaced), and the raced guard is
			// queued to raise when the victim is next displayed.
			victim := m.guard.cont.victim
			m.guard.cont = continuation{}
			m = m.clearGuardPrompt()
			m.opentabs = m.opentabs.SetDirty(victim, false)
			m = m.queueRacedGuard(victim.DocID, msg.Path, msg.Result.Saved, msg.Result.Fresh, &cmds)
			var noticeCmd tea.Cmd
			m.footer, noticeCmd = m.footer.Update(footer.ShowStatusMsg{
				Text: fmt.Sprintf("Eviction aborted — %q raced with a concurrent write; open it to resolve", filepath.Base(msg.Path)),
			})
			cmds = append(cmds, noticeCmd)
			return m, cmds, false
		}
		var openCmd tea.Cmd
		m, openCmd = m.evictSaveAck()
		cmds = append(cmds, openCmd)
		return m, cmds, false
	}
	// A materialize from the multi-tab quit "Save all" batch.
	if m.guard.cont.kind == contQuit && m.guard.cont.saveLeft > 0 {
		if msg.Result.Raced {
			// Critic R2: a swap-race during a quit-batch save must abort the
			// quit and surface it — never silently SetDirty(...,false) +
			// saveLeft-- straight through to teardownAndQuit while a
			// captured-but-unreconciled displaced write sits unseen. The
			// bytes are captured (I1), but the user must hear about the
			// race before the app exits.
			m.guard.cont = continuation{}
			m = m.clearGuardPrompt()
			m.opentabs = m.opentabs.SetDirty(opentabs.TabHandle{DocID: msg.DocID}, false)
			var noticeCmd tea.Cmd
			m.footer, noticeCmd = m.footer.Update(footer.ShowStatusMsg{
				Text: fmt.Sprintf("Quit aborted — %q raced with a concurrent write and needs resolving first", filepath.Base(msg.Path)),
			})
			cmds = append(cmds, noticeCmd)
			// surfaceRaced (W6): queue rather than raise when not displayed —
			// the guard's restore-theirs journals against the DISPLAYED
			// buffer, and the reopen below settles asynchronously; a guard
			// raised now would be refused as "no longer displayed" (review
			// finding 10's premature-guard scenario). drainRacedQueue raises
			// it the moment the reopen's load settles.
			displayed := msg.DocID == m.view.DocID()
			m = m.surfaceRaced(displayed, msg.DocID, msg.Path, msg.Result.Saved, msg.Result.Fresh, &cmds)
			if !displayed {
				var openCmd tea.Cmd
				m, openCmd = m.requestOpenPath(msg.DocID, msg.Path)
				cmds = append(cmds, openCmd)
			}
			return m, cmds, false
		}
		m.opentabs = m.opentabs.SetDirty(opentabs.TabHandle{DocID: msg.DocID}, false)
		m.guard.cont.saveLeft--
		if m.guard.cont.saveLeft == 0 {
			quitM, quitCmd := m.teardownAndQuit()
			return quitM, append(cmds, quitCmd), true
		}
		return m, cmds, false
	}
	// Orphaned quit-batch ack: the quit was already aborted (a Raced/diverged
	// sibling cleared guard.cont) but the OTHER batch saves were still
	// in flight — their acks land here with no matching branch above (review
	// finding: a SECOND race in the same batch fell through every handler
	// and vanished silently). The store side already committed in
	// Materialize's own tx; do the UI bookkeeping and surface any race.
	if strings.HasPrefix(msg.RequestID, "quitsave-") {
		m.opentabs = m.opentabs.SetDirty(opentabs.TabHandle{DocID: msg.DocID}, false)
		if msg.Result.Raced {
			// surfaceRaced (W6): raise-vs-queue on displayed identity.
			m = m.surfaceRaced(msg.DocID == m.view.DocID(), msg.DocID, msg.Path, msg.Result.Saved, msg.Result.Fresh, &cmds)
		}
		return m, cmds, false
	}
	return m, cmds, false
}

// handleFileSaveErrorMsg processes a FileSaveErrorMsg: keeps the buffer,
// routes a Missing/Conflict outcome for the current doc to the appropriate
// guard, and aborts any pending close/quit so nothing is discarded. Extracted
// from Update to keep workspace_update.go under the 500-LoC limit (§1.6/§11).
func (m Model) handleFileSaveErrorMsg(msg FileSaveErrorMsg, cmds []tea.Cmd) (Model, []tea.Cmd) {
	var cmd tea.Cmd
	// Interactive / bind-new save failure: keep the buffer, surface the
	// conflict, and abort any pending close so nothing is discarded — but
	// ONLY when the pending close is this save's own (GUARD-STATE-COH): a
	// bind-new triggered by finalizing the title can fail well after an
	// unrelated, later ^W has raised its own close guard for the SAME
	// keypress's synchronous continuation (title.Commit()'s RenameRequestMsg
	// is async — requestCloseCurrent runs before it lands). Clearing that
	// unrelated guard here would leave the footer showing a prompt with
	// nothing behind it.
	if m.activeSave.InFlight && m.activeSave.RequestID == msg.RequestID {
		m.activeSave.InFlight = false
		if m.guard.cont.owns(contClose, msg.RequestID) {
			m.guard.cont = continuation{}
			m = m.clearGuardPrompt()
		}
		var reopenCmd tea.Cmd
		m, reopenCmd = m.flushPendingReopen()
		cmds = append(cmds, reopenCmd)
		if msg.DocID == m.view.DocID() && m.view.IsFile() {
			// A vanished target (deleted/renamed since open) routes to the
			// file-deleted guard, not the merge guard — there is no "theirs"
			// to read or diff (§1.4.4: Materialize refused to silently
			// recreate it — see MatResult.Missing). This unifies save-time
			// deletion detection with the idle dirChangedMsg path; both raise
			// GuardDeleted, whose [S]ave force-recreates (bindNew=true).
			if msg.Missing {
				m = m.raiseDeletedGuard(msg.DocID, msg.Path)
				return m, cmds
			}
			// A conflict on the current doc launches the merge guard.
			// store.Materialize already captured the conflicting disk bytes
			// (I1) — raiseConflictGuard is pure SQLite, no async read needed.
			if msg.Conflict {
				var guardCmd tea.Cmd
				m, guardCmd = m.raiseConflictGuard(msg.DocID, msg.Path, m.editor.Content(), msg.Fresh.BlobHash, msg.Fresh.ID)
				cmds = append(cmds, guardCmd)
				return m, cmds
			}
		}
		// Non-conflict, non-missing errors (permissions, disk full, etc.)
		// still show the generic error banner. saveErrorText never calls
		// msg.Err.Error() on a nil Err (Conflict/Missing outcomes carry no
		// Err — a background tab's own conflict/missing target falling
		// through to here, since the msg.DocID==m.view.DocID() branch above
		// didn't match, would otherwise nil-panic — never panic, §1.3).
		m.footer, cmd = m.footer.Update(footer.ShowErrorMsg{Text: saveErrorText(msg)})
		cmds = append(cmds, cmd)
		return m, cmds
	}
	// A save in the multi-tab quit batch failed → abort the whole quit on the
	// first failure; every buffer is kept (durable in the VFS) and the
	// conflict is surfaced. Other in-flight saves still complete (their writes
	// succeeded); their acks are ignored now that the action is cleared.
	if m.guard.cont.kind == contQuit {
		m.guard.cont = continuation{}
		m = m.clearGuardPrompt()
		m.footer, cmd = m.footer.Update(footer.ShowErrorMsg{Text: saveErrorText(msg)})
		cmds = append(cmds, cmd)
	}
	// Eviction background save failed — the pending file does not open;
	// the victim tab stays open, the user can act on it manually.
	if m.guard.cont.owns(contEvict, msg.RequestID) {
		m.guard.cont = continuation{}
		m = m.clearGuardPrompt()
		m.footer, cmd = m.footer.Update(footer.ShowErrorMsg{Text: saveErrorText(msg)})
		cmds = append(cmds, cmd)
	}
	return m, cmds
}

// saveErrorText renders a FileSaveErrorMsg for the generic error banner
// fallback (a background tab's own conflict/missing-target, which the
// current-view-only guard-raising branches above don't route to a modal).
// Never dereferences a nil Err (§1.3 — never panic): Conflict and Missing
// outcomes carry no Err, only the discriminant.
func saveErrorText(msg FileSaveErrorMsg) string {
	switch {
	case msg.Err != nil:
		return msg.Err.Error()
	case msg.Missing:
		return fmt.Sprintf("save failed: %q was deleted", msg.Path)
	case msg.Conflict:
		return fmt.Sprintf("save failed: %q changed on disk", msg.Path)
	default:
		return fmt.Sprintf("save failed: %q", msg.Path)
	}
}
