package workspace

import (
	"fmt"
	"time"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/docstate"
	"rune/pkg/ui/components/footer"
	"rune/pkg/ui/components/opentabs"
	"rune/pkg/ui/pages/workspace/mergemode"
)

// currentSeqFor returns the journal position docID currently reflects, read
// SYNCHRONOUSLY (co-atomic with the content the caller captured) so a save
// tags its Materialize observation at the position the bytes it writes
// correspond to — never the live head a later edit advances to while the
// async write is in flight (§1.4.2/§1.4.8). 0 when there is no store/doc.
func (m Model) currentSeqFor(docID int64) int64 {
	if m.store == nil || docID == 0 {
		return 0
	}
	seq, err := m.store.CurrentSeq(docID)
	if err != nil {
		return 0 // fire-and-forget: read error → seq 0; conservative
	}
	return seq
}

// savedObsFor returns docID's current CAS expectation (the disk fact we
// believe is current), read SYNCHRONOUSLY at save-start alongside
// currentSeqFor and the live content — all three captured co-atomically so
// Materialize's CAS check and its resulting save observation both describe
// EXACTLY the state save-start saw, never a value a later edit could have
// moved out from under the in-flight write. ok=false (§1.7 — validity
// out-of-band, never an ObsID(0) sentinel) means there is no prior disk fact
// (no store, no doc, or the document has never been materialized/loaded);
// callers that bindNew=true (create/recreate) never consult the returned
// ObsID at all in that case — Materialize's create path never reads expect
// — but an overwrite-intent caller (bindNew=false) MUST check ok and refuse
// rather than pass a meaningless id into a CAS check (§1.3).
func (m Model) savedObsFor(docID int64) (docstate.ObsID, bool) {
	if m.store == nil || docID == 0 {
		return 0, false
	}
	obs, ok, err := m.store.SavedObs(docID)
	if err != nil || !ok {
		return 0, false
	}
	return obs.ID, true
}

// vetSaveOutcome is vetSave's result. Callers branch on it in this order —
// SyncErr, then Sync.Kind == docstate.SyncDiverged, then !HasExpect — with
// their OWN site-specific error text, guard-raising, and abort/skip
// semantics (startSave surfaces a footer error; evictSave also clears
// pendingDataLoss; saveAllDirtyForQuit aborts the whole quit and names the
// path); vetSave itself never decides what a caller does with a refusal.
type vetSaveOutcome struct {
	Sync      docstate.SyncState // valid when SyncErr == nil
	SyncErr   error
	Expect    docstate.ObsID // valid when HasExpect; the CAS baseline to Materialize against
	HasExpect bool           // false: no prior disk baseline (§1.7 — never an ObsID(0) sentinel)
}

// vetSave is the save-gate chokepoint shared by startSave, evictSave, and
// saveAllDirtyForQuit: it re-derives the two facts every overwrite-intent
// save must check FRESH before writing (§1.4.8 — never a flag cached from an
// earlier detection, always recomputed from the store on this transition):
//
//  1. Sync(docID) — Load unconditionally advances saved_obs to the latest
//     disk sighting it records, even a SyncDiverged conflict the user never
//     resolved, so once a guard is merely dismissed (Esc) CAS's own expect
//     can legitimately still match disk; this re-check is the only thing
//     that catches "what I last looked at was itself an unresolved
//     conflict" (CAS alone only proves "disk didn't move since I looked").
//  2. savedObsFor(docID) — the CAS baseline itself.
//
// A caller with bindNew=true (create/recreate) never needs this at all —
// Materialize's create path never reads expect.
func (m Model) vetSave(docID int64) vetSaveOutcome {
	// D2: mirrors savedObsFor's own nil-store guard (m.store.Sync would
	// otherwise dereference a nil *docstate.Store). Every current caller
	// already checks m.store == nil before reaching here, but that
	// invariant lives in the callers, not this function — surface a SyncErr
	// so a future/careless caller refuses the save (§1.3) instead of
	// panicking (a panic here would take the unsaved buffer with it).
	if m.store == nil {
		return vetSaveOutcome{SyncErr: fmt.Errorf("vetSave: no store")}
	}
	sync, syncErr := m.store.Sync(docID)
	if syncErr != nil {
		return vetSaveOutcome{SyncErr: syncErr}
	}
	if sync.Kind == docstate.SyncDiverged {
		return vetSaveOutcome{Sync: sync}
	}
	expect, ok := m.savedObsFor(docID)
	return vetSaveOutcome{Sync: sync, Expect: expect, HasExpect: ok}
}

// startSave is the ⌘S entry point: an ordinary interactive save request,
// never bypassing the degraded-store confirmation guard.
func (m Model) startSave() (Model, tea.Cmd) {
	return m.startSaveDegradedConfirmed(false)
}

// startSaveDegradedConfirmed runs the ordinary startSave sequence.
// degradedConfirmed is true ONLY when re-entering after the user answered
// [Y]es to the GuardDegraded prompt (handleDataLossConfirmDegraded) — every
// other caller passes false, so the guard is re-evaluated fresh on every
// independent save attempt (never "confirmed once, silently skipped for the
// rest of the session").
func (m Model) startSaveDegradedConfirmed(degradedConfirmed bool) (Model, tea.Cmd) {
	// Inert while a load is pending: the editor buffer may not yet match the
	// incoming identity (close→neighbour transition), so a save now could write
	// the wrong bytes. The gate clears on the load result (§1.4).
	// Not a file (untitled / help / transitional 0/""), a save in flight, or a load
	// pending → inert. !IsFile() also makes the read-only help structurally
	// non-saveable. The gate clears on the load result (§1.4).
	if !m.view.IsFile() || m.activeSave.InFlight || m.pendingLoad.active {
		return m, nil
	}
	// R2 save-gating while MERGING (BUG3): ⌘S with unresolved conflict blocks must
	// NOT re-raise the external-change guard ("File changed on disk" + [S]/[D]/[M]).
	// By the time the user is in merge mode the external change has already been
	// reconciled — handleDataLossMerge re-stamped the baseline to theirs and the
	// buffer is a valid, marker-free merge result, so the doc is legitimately just
	// *dirty*. Re-raising GuardMerge conflated "external change detected" with
	// "mid-merge"; instead surface a merge-resolution hint and leave the user in the
	// resolver. (Writing is gated until the blocks are resolved so theirs is never
	// silently dropped — §0; resolution itself is the markdownedit resolver UX.)
	if m.HasUnresolvedConflicts() {
		n := mergemode.ConflictsLeft(m.merge)
		var cmd tea.Cmd
		m.footer, cmd = m.footer.Update(footer.ShowStatusMsg{
			Text: fmt.Sprintf("%d conflict(s) to resolve — [O]urs / [T]heirs", n),
		})
		return m, cmd
	}
	if m.store == nil {
		var cmd tea.Cmd
		m.footer, cmd = m.footer.Update(footer.ShowErrorMsg{Text: "cannot save: storage not ready"})
		return m, cmd
	}
	// Degraded-store confirmation (verified below-cap item, WP-R4 item 5):
	// capture-into-RAM must never masquerade as durability. Asked BEFORE
	// every Materialize the interactive ⌘S path reaches, never bypassed by
	// an earlier confirmation in the same session.
	if m.store.Degraded() && !degradedConfirmed {
		m.footer = m.footer.SetGuard(footer.GuardDegraded, guardDegradedOptions)
		return m, nil
	}
	// §1.4.8: vetSave re-derives divergence + CAS baseline fresh at save-time
	// — never trust a flag cached from an earlier detection (see vetSave's
	// own doc comment for why this re-check is the guard CAS alone can't
	// provide).
	v := m.vetSave(m.view.DocID())
	if v.SyncErr != nil {
		// §1.3/WP-R4: a Sync error refuses the save with a surfaced error —
		// never falls through to write. The pre-save re-check is the ONLY
		// guard against writing over an Esc-dismissed divergence (CAS's
		// expect can legitimately match disk in that state), so skipping it
		// on error would silently clobber theirs exactly when the store is
		// least trustworthy (review finding).
		var cmd tea.Cmd
		m.footer, cmd = m.footer.Update(footer.ShowErrorMsg{Text: "cannot save: divergence check failed: " + v.SyncErr.Error()})
		return m, cmd
	}
	if v.Sync.Kind == docstate.SyncDiverged {
		var guardCmd tea.Cmd
		m, guardCmd = m.raiseConflictGuard(m.view.DocID(), m.view.Path(), m.editor.Content(), v.Sync.Theirs.Hash, v.Sync.Theirs.Obs)
		return m, guardCmd
	}
	// Overwrite an already-bound file: store.Materialize's own CAS check
	// (unconditional pre-write hash, Part III step 1-2) refuses if disk
	// diverged from the CAS expectation captured here — content-hash based,
	// no separate baseline/backstop plumbing needed (closes G3). This is an
	// overwrite-intent save (bindNew=false), so a missing CAS baseline
	// (§1.7 — no ObsID(0) sentinel) is refused outright rather than passed
	// into Materialize as a meaningless expect: the document is displayed as
	// a bound file, so SavedObs failing to produce a baseline here means
	// something is genuinely wrong, not "nothing to compare against yet".
	if !v.HasExpect {
		var cmd tea.Cmd
		m.footer, cmd = m.footer.Update(footer.ShowErrorMsg{Text: "cannot save: no prior disk baseline for this document"})
		return m, cmd
	}
	expect := v.Expect
	content := m.editor.Content()
	requestID := fmt.Sprintf("save-%v", time.Now().UnixNano())
	// Stamp pendingDataLoss with THIS save's requestID only when it's the
	// continuation of an existing pending actionClose (the confirmed
	// close-save flow, footer.DataLossSave -> startSave, reached with
	// pendingDataLoss still actionClose) — never when it's actionNone (plain
	// ⌘S; Priority 2.1's guard-in-progress gate already guarantees no other
	// key can raise a guard while this is reached with kind==actionNone).
	// MUST be an exact actionClose match, not merely != actionNone: an
	// eviction victim's background save (evictSave) never touches
	// activeSave/InFlight and resolves its OWN guard synchronously before
	// dispatching, so nothing blocks a totally ordinary, unrelated ⌘S on the
	// currently-displayed file while that eviction save is still in flight
	// with pendingDataLoss.kind==actionEvict — a broader `!= actionNone`
	// guard here would clobber pendingDataLoss.requestID with THIS save's ID,
	// breaking isEvictSaveAck's correlation and silently dropping the
	// eviction's own ack (review finding). This correlation is what lets
	// handleFileSaveErrorMsg/handleFileSavedMsg's ack handlers tell "this
	// save owns the pending guard" apart from "an unrelated guard happens to
	// be up right now" (GUARD-STATE-COH).
	if m.pendingDataLoss.kind == actionClose {
		m.pendingDataLoss.requestID = requestID
	}
	m.activeSave = SaveIdentity{
		RequestID:    requestID,
		SavedContent: []byte(content),
		InFlight:     true,
		Path:         m.view.Path(),
		DocID:        m.view.DocID(),
	}
	seq := m.currentSeqFor(m.view.DocID())
	return m, materializeStoreCmd(m.store, m.view.DocID(), m.view.Path(), content, expect, seq, requestID, false)
}

func (m Model) syncDirty() Model {
	if m.viewingHelp() || m.store == nil || m.view.DocID() == 0 {
		return m // no DB record — last-known mark stands
	}
	dirty, err := m.store.IsDirty(m.view.DocID())
	if err != nil {
		// fire-and-forget: dirty is a rung-3 display indicator; the journal is the durable truth
		return m
	}
	if dirty {
		m.opentabs = m.opentabs.MarkDirtyByID(m.view.DocID())
	} else {
		m.opentabs = m.opentabs.MarkCleanByID(m.view.DocID())
	}
	return m
}

func (m Model) finalize(cmds []tea.Cmd) (Model, tea.Cmd) {
	m = m.syncDirty()
	// TAB-SET: mirror the active tab to the live identity. During a close→neighbour
	// transition the live identity is the save-safe 0/"" (executeClose left it
	// there), so derive the active tab + link base dir from the pending target
	// instead — the active handle intentionally LEADS the identity by one async
	// hop here (INV-ACTIVE-SYNC holds only after settle; do NOT "fix" it by
	// tracking live identity — that reintroduces the stranding). Every other load
	// path keeps a non-empty live identity, so it is unaffected.
	active := m.view.Handle()
	if m.pendingLoad.active && m.view.IsUntitled() && m.view.DocID() == 0 {
		active = opentabs.TabHandle{DocID: m.pendingLoad.docID, Path: m.pendingLoad.path}
	}
	m.opentabs = m.opentabs.SetActive(active)
	// Project the editor's link/embed base from the single source (m.view) at this
	// one authority point — like applyFocus projects m.focus. The GOLDEN path
	// verbatim (the editor derives Dir() itself), so it tracks every settled
	// transition (load/untitled/help/bind/rename) and never drifts.
	m.editor = m.editor.SetDocPath(m.view.Path())
	m = m.applyFocus()
	if m.totalWidth > 0 {
		m = m.recalcLayout()
	}
	return m, tea.Batch(cmds...)
}

func (m Model) finalizeLayoutChange(cmds []tea.Cmd) (Model, tea.Cmd) {
	m = m.applyFocus()
	if m.totalWidth > 0 {
		m = m.recalcLayout()
		var refreshCmd tea.Cmd
		m.editor, refreshCmd = m.editor.RefreshImagesAfterLayoutChange()
		if refreshCmd != nil {
			cmds = append(cmds, refreshCmd)
		}
	}
	return m, tea.Batch(cmds...)
}

// setFocus is the only sanctioned way to change m.focus. It atomically sets the
// focus enum and projects it onto every child via applyFocus, so a bare
// m.focus = x that skips the projection is impossible by construction.
func (m Model) setFocus(p pane) Model {
	m.focus = p
	return m.applyFocus()
}

// applyFocus projects the single focus authority (m.focus) onto every child's
// focus state. Called by setFocus and as a safety-net at every Update exit.
func (m Model) applyFocus() Model {
	m.title = m.title.SetFocused(m.focus == paneTitle)
	m.filetree = m.filetree.SetFocused(m.focus == paneTree)
	m.opentabs = m.opentabs.SetFocused(m.focus == paneTabs)
	m.editor = m.editor.SetFocused(m.focus == paneCenter)
	m.chat = m.chat.SetFocused(m.focus == paneChat)
	m.search = m.search.SetFocused(m.focus == paneSearch)
	return m
}

func (m Model) syncCursorToFooter() Model {
	// Surface the link under the caret as a footer hint, but only while the
	// editor is focused (the caret is meaningless to the user otherwise).
	linkTarget := ""
	if m.focus == paneCenter {
		linkTarget, _ = m.editor.LinkAtCursor()
	}
	m.footer, _ = m.footer.Update(footer.UpdateCursorMsg{LinkTarget: linkTarget})
	return m
}

func (m Model) syncDictationAllowed() Model {
	m.footer = m.footer.SetDictationAllowed(m.focus == paneCenter || m.focus == paneChat)
	return m
}

// disableDictationForTransition stops any active dictation session before a
// buffer-identity transition (tab switch/load, undo/redo, conflict
// resolution) that a stale dictation anchor could otherwise corrupt. H1: the
// anchor (startOff/appliedLen, fixed at Enable) targets whatever buffer is
// displayed when the NEXT chunk lands — not necessarily the one dictation
// started against. dict.Disable() previously existed at only 3 sites
// (merge-gate routeDictationEdit, user stop, quit); this is the shared helper
// for the sites the transitions below add. A no-op (no footer notice queued)
// when no session is active, so it is safe to call unconditionally.
func (m Model) disableDictationForTransition(cmds *[]tea.Cmd) Model {
	if !m.dict.Enabled() {
		return m
	}
	m.dict = m.dict.Disable()
	m.footer = m.footer.SetDictating(false)
	*cmds = append(*cmds, func() tea.Msg {
		return footer.ShowStatusMsg{Text: "Dictation stopped — document changed"}
	})
	return m
}

// syncMergeHint mirrors the merge resolver's active/left-count onto the
// footer so the persistent "[O]urs [T]heirs ... N left" hint (§5) always
// reflects the current mergemode.State. Called from the same three points as
// syncCursorToFooter (workspace_update_keys.go, workspace_update.go,
// workspace_edit.go: handleUndo/handleRedo).
func (m Model) syncMergeHint() Model {
	m.footer = m.footer.SetMergeMode(mergemode.IsActive(m.merge), mergemode.ConflictsLeft(m.merge))
	return m
}

// errorCmd surfaces err on the footer status line. Per §1.3 a buffer-edit failure
// is a Tolerable halt that keeps the buffer — never a silent drop.
func errorCmd(err error) tea.Cmd {
	text := err.Error()
	return func() tea.Msg { return footer.ShowErrorMsg{Text: text} }
}

// undoTarget, handleUndo, and handleRedo live in workspace_undo.go (§1.6,
// split out of this file to stay under the 500-LoC limit).
//
// scheduleFlush, snapshotCmd, journalEdit, and rollbackFailedJournal (the
// journal/autosave plumbing) live in workspace_journal.go.
