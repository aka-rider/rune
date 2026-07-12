package workspace

// Fix A / Fix B (ACTIVE plan): the merge/discard/undo-unwind actions must act
// on FRESH disk state taken at the decisive moment, never on bytes cached at
// conflict-DETECTION time or trust a baseline invalidated at undo time. Both
// fixes share the same two-phase async pattern: the key press / journal jump
// captures what it can synchronously, launches a fresh store.Probe, and
// applies the result only once it lands — never a blocking read in Update
// (§5.3). WP5: Probe (docstate's disk layer) replaces the raw vfs.FS reads
// this package used to issue directly.

import (
	"errors"
	"fmt"
	"io/fs"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/docstate"
	"rune/pkg/editor/buffer"
	"rune/pkg/merge"
	"rune/pkg/ui/components/footer"
	"rune/pkg/ui/pages/workspace/mergemode"
)

// ---- Fix A: [M]/[D] act on a FRESH probe, detect deletion early -----------

// mergeIntent distinguishes what a resolveProbeMsg's fresh probe is for.
type mergeIntent int

const (
	mergeIntentMerge mergeIntent = iota
	mergeIntentDiscard
)

// resolveProbeMsg is returned by resolveProbeCmd once the fresh disk state
// for a pending [M]/[D] action has been probed. ours is captured
// SYNCHRONOUSLY at the key press (before the probe is issued) so a
// dictation/paste edit already in the buffer is never lost (mirrors
// handleDataLossSaveAnyway's live-buffer read) — only theirs is stale-prone,
// so only theirs is re-probed fresh here. ticket is the view-targeted result
// ticket (Part IV) captured at the key press, alongside ours.
type resolveProbeMsg struct {
	ticket  viewTicket
	path    string
	ours    string
	intent  mergeIntent
	state   docstate.SyncState
	missing bool
	err     error
}

// resolveProbeCmd re-probes ticket.docID's disk state at the moment of a
// [M]/[D] action — the fix for the merge data-race (theirs read once at
// detection, used stale at the action). A probe error or a vanished file is
// surfaced to handleResolveProbe, which never merges/discards against stale
// or absent bytes.
func resolveProbeCmd(store *docstate.Store, ticket viewTicket, path, ours string, intent mergeIntent) tea.Cmd {
	return func() tea.Msg {
		state, err := store.Probe(ticket.docID)
		if err != nil {
			if errors.Is(err, fs.ErrNotExist) {
				return resolveProbeMsg{ticket: ticket, path: path, ours: ours, intent: intent, missing: true}
			}
			return resolveProbeMsg{ticket: ticket, path: path, ours: ours, intent: intent, err: err}
		}
		return resolveProbeMsg{ticket: ticket, path: path, ours: ours, intent: intent, state: state}
	}
}

// handleResolveProbe applies the result of resolveProbeCmd through
// applyViewResult (Part IV — the single chokepoint for a view-targeted async
// result): a stale ticket (the doc switched, or a load/undo/resolve replaced
// the buffer while this probe was in flight) is refused with a footer notice
// rather than applied to whatever the user is now looking at — this is
// STRICTLY narrower than the pre-WP6 docID-only recheck (H2's original fix),
// since it also catches "same doc, but the buffer moved on" (e.g. an undo
// landing between the [M] press and this probe's return). A vanished target
// or a probe error means there is no "theirs" to merge or discard against —
// route to the deleted guard EARLY (at the [M]/[D] press), rather than
// discovering it late at ⌘S.
func (m Model) handleResolveProbe(msg resolveProbeMsg) (Model, tea.Cmd) {
	return m.applyViewResult(msg.ticket, func(m Model) (Model, tea.Cmd) {
		if msg.err != nil || msg.missing {
			m.merge = mergemode.Reset(m.merge)
			m = m.raiseDeletedGuard(msg.ticket.docID, msg.path)
			return m, nil
		}
		theirsContent, err := m.blobFor(msg.state.Theirs)
		if err != nil {
			// F4/§1.3: theirs is ALWAYS Valid here (a fresh Probe always
			// populates it) — a read failure means a corrupt/unreadable
			// blob, never a legitimate absence. Refuse the resolution
			// outright rather than discarding/merging against
			// substituted-empty content; the buffer stays exactly as-is.
			return m, errorCmd(fmt.Errorf("resolve %q: %w", msg.path, err))
		}
		switch msg.intent {
		case mergeIntentDiscard:
			return m.applyDiscardConflict(msg.ticket.docID, theirsContent, msg.state)
		default:
			ancestorContent, err := m.blobFor(msg.state.Ancestor)
			if err != nil {
				return m, errorCmd(fmt.Errorf("resolve %q: %w", msg.path, err))
			}
			return m.applyMergeConflict(msg.ticket.docID, msg.path, msg.ours, ancestorContent, theirsContent, msg.state)
		}
	})
}

// blobFor resolves a docstate.Version to its content. A Version with
// Valid==false (or no store) legitimately carries no blob — returns ("",
// nil), which callers may treat as "absent" (e.g. no ancestor recorded yet,
// a valid 2-way situation). A Version WITH a hash whose blob fails to read
// (corrupt content — GetBlob's own SHA-256 re-verification failing — or the
// row simply gone) is a REAL error: every caller MUST surface it and refuse
// the action rather than silently substituting "" for real content (F4,
// §1.3 — a conflict resolution never proceeds with substituted-empty
// content, the named forbidden case).
func (m Model) blobFor(v docstate.Version) (string, error) {
	if !v.Valid || m.store == nil {
		return "", nil
	}
	content, err := m.store.GetBlob(v.Hash)
	if err != nil {
		return "", fmt.Errorf("read blob %s: %w", v.Hash, err)
	}
	return content, nil
}

// applyDiscardConflict replaces the editor buffer with the FRESH theirs bytes
// (the external version, re-probed at the [D] press), journals the
// ReplaceAll synchronously (S7 — mirrors applyMergeConflict, closing the
// window where recovery could reconstruct pre-discard content over what the
// user sees), and commits the resolution via ResolveAdopt so the CAS
// baseline advances to theirs and undoing past this point re-exposes the
// divergence (Part III conflict lifecycle).
func (m Model) applyDiscardConflict(docID int64, theirs string, sync docstate.SyncState) (Model, tea.Cmd) {
	var cmds []tea.Cmd
	// H1: dictation must not survive a discard — its anchor targeted the
	// pre-discard buffer.
	m = m.disableDictationForTransition(&cmds)

	prevCursors := m.editor.Cursors()
	var cmd tea.Cmd
	var err error
	m.editor, cmd, err = m.editor.ReplaceAll(theirs)
	if err != nil {
		cmds = append(cmds, errorCmd(fmt.Errorf("discard: %w", err)))
		return m, tea.Batch(cmds...)
	}
	cmds = append(cmds, cmd)
	m = m.bumpEpoch() // Part IV: a discard resolve ReplaceAll invalidates every outstanding view ticket
	m.merge = mergemode.Reset(m.merge)

	var editorEdits []buffer.AppliedEdit
	m.editor, editorEdits = m.editor.DrainEdits()
	if len(editorEdits) == 0 {
		// D3: mirrors installDiskAhead's own gate — a read-only editor drops
		// ReplaceAll silently (textedit.ReplaceRange's readOnly guard), so the
		// buffer still holds pre-discard ours even though ReplaceAll above
		// returned no error. Advancing the CAS baseline to theirs via
		// resolveAdoptAt below would then bless a later ⌘S that writes ours
		// over theirs while claiming the discard already reconciled it.
		cmds = append(cmds, errorCmd(fmt.Errorf("discard refused for doc %d: editor rejected the install (read-only?)", docID)))
		return m, tea.Batch(cmds...)
	}
	var journaled bool
	m, journaled = m.journalEditOK("main", editorEdits, prevCursors, m.editor.Cursors(), &cmds)
	if !journaled {
		// journalEditOK already rolled the buffer back and surfaced the
		// error — the adoption below must not proceed on a journal write
		// that never landed (same reasoning as installDiskAhead).
		return m, tea.Batch(cmds...)
	}

	m = m.resolveAdoptAt(docID, sync, &cmds)
	m = m.setDiskChangedHint(false)
	return m, tea.Batch(cmds...)
}

// applyMergeConflict runs MergeHunks on ancestor/ours/FRESH-theirs (re-probed
// at the [M] press) and enters the resolver.
func (m Model) applyMergeConflict(docID int64, path, ours, ancestor, theirs string, sync docstate.SyncState) (Model, tea.Cmd) {
	hunks, err := merge.MergeHunks([]byte(ancestor), []byte(ours), []byte(theirs))
	if err != nil {
		var cmd tea.Cmd
		m.footer, cmd = m.footer.Update(footer.ShowErrorMsg{
			Text: fmt.Sprintf("merge failed: %v — use [S]ave anyway or [D]iscard", err),
		})
		var guardCmd tea.Cmd
		m, guardCmd = m.raiseConflictGuard(docID, path, ours, sync.Theirs.Hash, sync.Theirs.Obs)
		return m, tea.Batch(cmd, guardCmd)
	}

	var cmds []tea.Cmd
	// H1: dictation must not survive entering the merge resolver — its anchor
	// targeted the pre-merge buffer, and mid-merge the live buffer becomes
	// mergemode's hidden marker working form (routeDictationEdit's gate covers
	// the STEADY-state merge; this covers the transition INTO it).
	m = m.disableDictationForTransition(&cmds)
	var cmd tea.Cmd
	prevCursors := m.editor.Cursors()
	m.merge, m.editor, cmd, err = mergemode.Enter(hunks, m.merge, m.editor)
	if err != nil {
		cmds = append(cmds, errorCmd(fmt.Errorf("merge: %w", err)))
		return m, tea.Batch(cmds...)
	}
	cmds = append(cmds, cmd)
	m = m.bumpEpoch() // Part IV: entering the merge resolver installs the marker buffer, invalidating every outstanding view ticket
	var editorEdits []buffer.AppliedEdit
	m.editor, editorEdits = m.editor.DrainEdits()
	m = m.syncMergeHint()
	if len(editorEdits) == 0 {
		// D3: mirrors installDiskAhead's own gate — a read-only editor drops
		// mergemode.Enter's marker-buffer install silently (it routes through
		// the same textedit.ReplaceRange readOnly guard), so the buffer still
		// holds pre-merge ours even though Enter above returned no error.
		// Advancing the CAS baseline to theirs via resolveAdoptAt below would
		// then bless a later ⌘S that writes ours over theirs while claiming
		// the merge already reconciled it.
		cmds = append(cmds, errorCmd(fmt.Errorf("merge refused for doc %d: editor rejected the install (read-only?)", docID)))
		return m, tea.Batch(cmds...)
	}
	var journaled bool
	m, journaled = m.journalEditOK("main", editorEdits, prevCursors, m.editor.Cursors(), &cmds)
	if !journaled {
		// journalEditOK already rolled the buffer back and surfaced the
		// error — the adoption below must not proceed on a journal write
		// that never landed (same reasoning as installDiskAhead).
		return m, tea.Batch(cmds...)
	}

	// Re-stamp the CAS baseline to theirs so a resolved-merge ⌘S sees a clean
	// expect and writes cleanly — mirrors [D] (applyDiscardConflict).
	m = m.resolveAdoptAt(docID, sync, &cmds)
	m = m.setDiskChangedHint(false)

	return m, tea.Batch(cmds...)
}

// resolveAdoptAt commits the just-journaled resolution edit (the caller's
// ReplaceAll/marker-buffer install, already drained+journaled) via
// ResolveAdopt: sync.Theirs is the fresh disk observation this resolution
// reconciles against; the edit's own seq (read back synchronously — the
// journal write above just committed it, single-threaded within Update) is
// what the resolve observation correlates to, so undoing past it re-exposes
// the divergence (Part III).
func (m Model) resolveAdoptAt(docID int64, sync docstate.SyncState, cmds *[]tea.Cmd) Model {
	if m.store == nil || !sync.Theirs.Valid {
		return m
	}
	editSeq, err := m.store.CurrentSeq(docID)
	if err != nil {
		*cmds = append(*cmds, errorCmd(fmt.Errorf("resolve: read journal position: %w", err)))
		return m
	}
	if err := m.store.ResolveAdopt(docID, sync.Theirs.Obs, editSeq); err != nil {
		*cmds = append(*cmds, errorCmd(fmt.Errorf("resolve adopt: %w", err)))
	}
	return m
}

// abandonMergeResolve reverses the resolve observation that entering the
// merge resolver recorded (resolveAdoptAt, called by applyMergeConflict/
// applyDiscardConflict) — the workspace half of an Esc-abort out of the
// merge resolver (F3). store.ResolveAbandon restores docID's CAS baseline to
// EXACTLY what it was before the resolution (the supersedes chain — never
// re-derived by an origin scan an intervening sighting could poison), so the
// buffer mergemode.Abort just reverted to pre-merge ours (journaled by the
// caller right after this returns) is judged against the ORIGINAL disk fact
// again. A fresh Probe follows — reusing probeUnwindCmd/handleUnwindProbe,
// the SAME async re-detection the undo-unwind path already uses (§4) — so
// the Diverged guard re-raises immediately if the external change is still
// there, rather than waiting for the next idle probe tick or ⌘S's own
// re-check to discover it.
func (m Model) abandonMergeResolve(cmds *[]tea.Cmd) Model {
	if m.store == nil {
		return m
	}
	docID := m.view.DocID()
	if docID == 0 {
		return m
	}
	if err := m.store.ResolveAbandon(docID); err != nil {
		*cmds = append(*cmds, errorCmd(fmt.Errorf("abandon merge resolve: %w", err)))
		return m
	}
	*cmds = append(*cmds, probeUnwindCmd(m.store, m.currentTicket(), m.view.Path()))
	return m
}

// installDiskAhead applies R1's disk-ahead auto-adopt as a REAL journaled
// transition (F1 — handleFileLoadedMsg's SyncDiskAhead branch): the buffer is
// first set to ours (docID's own journal reconstruction, matching every
// other load branch's baseline — the live editor may still be showing an
// UNRELATED tab's content right up until this call, since the async load
// result can land after the user switched away again) so the subsequent
// journaled ReplaceAll(theirs) diffs correctly against docID's OWN prior
// content, never a stale unrelated buffer. resolveAdoptAt then advances
// saved_obs to theirs at the edit's own seq — mirrors applyDiscardConflict's
// ours->theirs journaled install, but for the load-time (no conflict guard)
// case. After this call: store.Content(docID) == the displayed buffer, and a
// later quit/evict/second revisit can never write the stale pre-adopt
// reconstruction back over newer disk.
func (m Model) installDiskAhead(docID int64, ours, theirs string, sync docstate.SyncState, cmds *[]tea.Cmd) Model {
	m.editor = m.editor.SetContent(ours)
	prevCursors := m.editor.Cursors()
	var cmd tea.Cmd
	var err error
	m.editor, cmd, err = m.editor.ReplaceAll(theirs)
	if err != nil {
		*cmds = append(*cmds, errorCmd(fmt.Errorf("disk-ahead adopt: %w", err)))
		return m
	}
	*cmds = append(*cmds, cmd)
	var editorEdits []buffer.AppliedEdit
	m.editor, editorEdits = m.editor.DrainEdits()
	if len(editorEdits) == 0 {
		// The ReplaceAll never applied — a read-only editor drops it
		// silently (textedit.ReplaceRange's readOnly guard), and DiskAhead
		// guarantees ours != theirs so a legitimate no-op is impossible.
		// Adopting anyway would move the CAS baseline to theirs while
		// buffer and journal still hold ours — the exact F1 clobber this
		// function exists to close, reintroduced on this path (review
		// finding). Refuse + surface; Sync keeps reporting DiskAhead, so
		// the divergence stays visible instead of being memory-holed.
		*cmds = append(*cmds, errorCmd(fmt.Errorf("disk-ahead adopt refused for doc %d: editor rejected the install (read-only?)", docID)))
		return m
	}
	var ok bool
	m, ok = m.journalEditOK("main", editorEdits, prevCursors, m.editor.Cursors(), cmds)
	if !ok {
		// AppendEdit failed: journalEditOK already rolled the buffer back to
		// ours and surfaced the error. The adoption MUST not proceed — the
		// journal cannot reproduce theirs at any seq (Adoption Contract),
		// so a resolve observation would bless a ⌘S that writes the stale
		// reconstruction over the newer external content (review finding).
		return m
	}
	m = m.resolveAdoptAt(docID, sync, cmds)
	return m
}

// ---- Fix B: undo-unwind re-detects via Probe+Sync, not bespoke logic ------

// resyncMergeIfMain re-derives the merge resolver state after an undo/redo
// journal jump on "main" — a jump that did not go through mergemode.HandleKey,
// so the merge view (and active/blocks state) would otherwise drift from the
// buffer it now reflects (§4). Called unconditionally on every "main" jump
// (not gated on IsActive): a FULLY-RESOLVED merge deactivates without
// clearing mergemode.State's conflict list (only Reset/Abort do), so undoing
// the final accept must be able to REOPEN the block even though the resolver
// reads inactive right now. Resync itself is a cheap no-op when no merge was
// ever entered (empty conflict list).
//
// If the jump unwinds past mergemode.Enter's marker-load edit (active → not
// active), Fix B launches an ASYNC fresh Probe (probeUnwindCmd) rather than
// invalidating a cached baseline outright: WP5 replaces the old bespoke
// handleMergeUnwindRead decision logic with Probe+Sync — ancestorAt is
// derived from journal position (Part III), so undoing past a ResolveAdopt's
// correlated seq makes Sync report Diverged again structurally, with no
// special-cased re-detection needed here at all.
func (m Model) resyncMergeIfMain(target string) (Model, tea.Cmd) {
	if target != "main" {
		return m, nil
	}
	wasActive := mergemode.IsActive(m.merge)
	m.merge = mergemode.Resync(m.merge, m.editor)
	if wasActive && !mergemode.IsActive(m.merge) {
		return m, probeUnwindCmd(m.store, m.currentTicket(), m.view.Path())
	}
	return m, nil
}

// unwindProbeMsg is returned by probeUnwindCmd once the fresh disk state has
// been probed after an undo/redo unwound mergemode active→inactive. ticket
// is the view-targeted result ticket (Part IV) captured when the probe was
// issued (post-bumpEpoch, since the undo/redo buffer install already
// happened by the time resyncMergeIfMain runs).
type unwindProbeMsg struct {
	ticket  viewTicket
	path    string
	state   docstate.SyncState
	missing bool
	err     error
}

// probeUnwindCmd probes ticket.docID's disk state after an undo/redo journal
// jump takes the merge resolver from active to inactive (§4's
// resyncMergeIfMain).
func probeUnwindCmd(store *docstate.Store, ticket viewTicket, path string) tea.Cmd {
	if store == nil {
		return nil
	}
	return func() tea.Msg {
		state, err := store.Probe(ticket.docID)
		if err != nil {
			if errors.Is(err, fs.ErrNotExist) {
				return unwindProbeMsg{ticket: ticket, path: path, missing: true}
			}
			return unwindProbeMsg{ticket: ticket, path: path, err: err}
		}
		return unwindProbeMsg{ticket: ticket, path: path, state: state}
	}
}

// handleUnwindProbe applies the result of probeUnwindCmd (Fix B — BUG3):
// Sync's classification (via Probe) already re-derives whether the
// undo-unwound buffer still agrees with disk — Clean means nothing to
// reconcile; anything else re-raises the conflict guard on FRESH theirs, so
// a later ⌘S can never silently write pre-merge ours over theirs. The
// mergemode.IsActive recheck is a SEPARATE concern from ticket staleness (a
// redo could have re-entered merge before this probe landed) and is checked
// first; applyViewResult (Part IV chokepoint) then refuses a genuinely stale
// ticket — any later transition that changed the doc or replaced the buffer
// — rather than applying it to whatever the user is now looking at.
func (m Model) handleUnwindProbe(msg unwindProbeMsg) (Model, tea.Cmd) {
	if mergemode.IsActive(m.merge) {
		return m, nil
	}
	return m.applyViewResult(msg.ticket, func(m Model) (Model, tea.Cmd) {
		if msg.err != nil {
			return m, nil // fire-and-forget: degrade safely, a later probe/save re-derives
		}
		if msg.missing {
			m = m.raiseDeletedGuard(msg.ticket.docID, msg.path)
			return m, nil
		}
		if msg.state.Kind == docstate.SyncClean {
			return m, nil // nothing left to reconcile
		}
		ours := m.editor.Content()
		var cmd tea.Cmd
		m, cmd = m.raiseConflictGuard(msg.ticket.docID, msg.path, ours, msg.state.Theirs.Hash, msg.state.Theirs.Obs)
		return m, cmd
	})
}

// ---- Fix E1: dictation must never mutate the hidden marker buffer ---------

// routeDictationEdit applies a drained dictation pending edit (s, e, t) to the
// focused target, EXCEPT while a merge is active and the editor is focused:
// the main editor's live buffer is then mergemode's hidden marker working
// form (§3), whose byte offsets are exactly what mergemode's block-span
// tracking keys off of. A dictation edit landing there would mutate that
// buffer uncontrolled — the same hazard a stray keystroke would cause, which
// mergemode.HandleKey already refuses via its "no free editing while merging"
// default case — desyncing the block spans and corrupting which bytes
// [O]/[T] next collapse a block to. So mid-merge this stops the dictation
// session outright rather than silently dropping every future chunk forever
// with no feedback (E1, data safety).
//
// Part IV structural backstop: m.dict.Ticket() (docID/epoch captured at
// Enable) is validated against the CURRENT target before applying — the
// scattered Disable() calls at every known transition (workspace_edit.go /
// workspace_merge_fresh.go) are the fast, immediate UI-feedback path; this
// ticket check is what makes a stale chunk STRUCTURALLY refused even if a
// future transition forgets to call Disable() explicitly.
func (m Model) routeDictationEdit(s, e int, t string, cmds *[]tea.Cmd) Model {
	if m.focus == paneCenter && mergemode.IsActive(m.merge) {
		m.dict = m.dict.Disable()
		m.footer = m.footer.SetDictating(false)
		return m
	}
	ticketDocID, ticketEpoch := m.dict.Ticket()
	switch m.focus {
	case paneCenter:
		if ticketDocID != m.view.DocID() || ticketEpoch != m.epoch {
			m.dict = m.dict.Disable()
			m.footer = m.footer.SetDictating(false)
			return m
		}
		prevCursors := m.editor.Cursors()
		var cmd tea.Cmd
		var err error
		m.editor, cmd, err = m.editor.ReplaceRange(s, e, t)
		if err != nil {
			*cmds = append(*cmds, errorCmd(fmt.Errorf("dictation edit: %w", err)))
			return m
		}
		*cmds = append(*cmds, cmd)
		var dictEdits []buffer.AppliedEdit
		m.editor, dictEdits = m.editor.DrainEdits()
		m = m.journalEdit("main", dictEdits, prevCursors, m.editor.Cursors(), cmds)
	case paneChat:
		if ticketDocID != m.chatDocID {
			m.dict = m.dict.Disable()
			m.footer = m.footer.SetDictating(false)
			return m
		}
		var err error
		m.chat, err = m.chat.ApplyToPrompt(s, e, t)
		if err != nil {
			*cmds = append(*cmds, errorCmd(fmt.Errorf("dictation edit: %w", err)))
		}
	}
	return m
}
