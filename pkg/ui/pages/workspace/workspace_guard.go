package workspace

import "rune/pkg/ui/components/footer"

// guardKind is workspace's own semantic classification of which guard is
// showing — finer-grained than footer.GuardKind, which selects only what
// PROMPTS (F12/critic R5): guardDirtyClose/guardDirtyEvict/guardDirtyQuit all
// render as the identical GuardDirty prompt (same footer.GuardKind, same
// options), but are three semantically distinct reasons the guard was raised
// — guard.close/guard.evict/guard.quit (A4) track which one, independently
// of kind (critic R1: kind can flip to guardConflict on top of a live
// close/evict/quit intent, see guardState's own doc comment) — and every
// SetGuard call site already knows WHY it's raising a guard, so naming that
// reason here (rather than only picking a footer.GuardKind) keeps workspace's
// and footer's taxonomies aligned rather than introducing a second one.
type guardKind int

const (
	guardNone guardKind = iota
	guardTrash
	guardConflict
	guardDeleted
	guardRaced
	guardDegraded
	guardDirtyClose
	guardDirtyEvict
	guardDirtyQuit
)

// guardPhase is the workspace-owned lifecycle stage of the CURRENT guard
// prompt/continuation (A3/A4 design notes: "the new machine needs phases
// guardIdle | guardPrompting | guardAwaitingSave" — the close/evict/quit
// intents outlive the prompt across an async close/evict/quit save
// round-trip). A3 landed guardIdle/guardPrompting (every
// conflict/deleted/raced/trash/degraded resolution clears synchronously,
// never lingering past its own prompt); guardAwaitingSave is A4's addition:
// confirmGuardSave() moves a close/evict/quit guard here the instant its
// [S]ave response launches the background Materialize, so the prompt is
// gone (footer.InGuard() reads false) while guard.kind and the surviving
// close/evict/quit intent still identify the continuation FileSavedMsg/
// FileSaveErrorMsg must correlate against, one or more messages later.
type guardPhase int

const (
	guardIdle guardPhase = iota
	guardPrompting
	guardAwaitingSave
)

// guardState is workspace's semantic guard machine — a MULTI-FIELD struct,
// NOT a single-payload union (critic R1): kind/phase track ONLY what is
// currently prompting on the footer; the intent sub-structs (conflict/
// deleted/raced/close/evict/quit) are independently populated and may
// outlive kind switching to a DIFFERENT guard (e.g. a conflict guard raised
// mid-close/evict/quit-save does not erase that continuation's own intent —
// see raiseConflictGuard's callers). kind's zero value (guardNone) is
// meaningful — no raise call site ever sets it — so an "unrepresentable no
// guard" state never has to be faked with a sentinel.
//
// A4 note: resolving a conflict/deleted/trash/raced/degraded guard that was
// raised ON TOP OF a live close/evict/quit continuation UNCONDITIONALLY
// clears close/evict/quit too (handleDataLossSaveAnyway/
// handleDataLossDiscardConflict/handleDataLossMerge, handleDeletedSave/
// handleDeletedDiscard, the shared Cancel branch) — this ABANDONS the
// continuation rather than resuming it once the OTHER guard is dealt with;
// there is no re-arm anywhere in the codebase. Pinned by
// TestConflictDuringCloseSave_CoexistsThenAbandonsClose
// (workspace_guard_test.go).
type guardState struct {
	kind      guardKind
	phase     guardPhase
	trashPath string         // A3: migrated from the former pendingDataLoss.pendingTrashPath field (actionTrash removed) — the file to trash, valid while kind==guardTrash
	conflict  conflictIntent // A3: migrated from the former Model.pendingConflict field
	deleted   deletedIntent  // A3: migrated from the former Model.pendingDeleted field
	raced     racedIntent    // A3: migrated from the former Model.pendingRaced field; racedQueue (workspace.go) stays a separate map[int64]racedIntent — a deferred intent, not active guard state
	close     closeIntent    // A4: migrated from the former Model.pendingDataLoss{kind:actionClose} (workspace_nav.go)
	evict     evictIntent    // A4: migrated from the former Model.pendingDataLoss{kind:actionEvict} (workspace_evict.go)
	quit      quitIntent     // A4: migrated from the former Model.pendingDataLoss{kind:actionQuit} (workspace_quit.go)
}

// isCloseSaveAck reports whether requestID is the ack for the save startSave
// launched as the confirmed continuation of a live close intent
// (footer.DataLossSave -> startSave, which stamps guard.close.requestID —
// workspace_edit.go). An UNSTAMPED close intent (requestID=="") is a guard
// that some OTHER, unrelated save's ack must never clear — GUARD-STATE-COH: a
// save with no idea a close intent exists (bind-new, restore-theirs) can
// never accidentally "own" and clear someone else's still-pending guard.
// A4: moved onto guardState from Model (was isCloseSaveAck(requestID string)).
func (g guardState) isCloseSaveAck(requestID string) bool {
	return g.close.active && g.close.requestID != "" && requestID == g.close.requestID
}

// isEvictSaveAck reports whether requestID is the ack for the pending
// eviction background save. A4: moved onto guardState from Model.
func (g guardState) isEvictSaveAck(requestID string) bool {
	return g.evict.active && g.evict.requestID != "" && requestID == g.evict.requestID
}

// abandonDirtyContinuation wholesale-clears every close/evict/quit intent —
// the A4 equivalent of the pre-A4 `m.pendingDataLoss = pendingDataLoss{}`
// reset, which zeroed a SINGLE shared field regardless of which of
// close/evict/quit it currently held. Because that single field is now three
// independent guardState fields, every site that used to reach for the old
// wholesale reset (the shared Cancel branch, the conflict/deleted guard
// resolution handlers abandoning a coexisting close/evict/quit continuation
// per critic R1, teardownAndQuit) clears all three here instead — reproducing
// the exact same "whatever was pending is gone now" semantics without having
// to first ask which one it was. Does NOT touch guard.kind/phase — callers
// that also need the prompt/kind reset call clearGuardPrompt separately
// (mirrors the old code, where the single pendingDataLoss reset never touched
// guard.kind either).
func (m Model) abandonDirtyContinuation() Model {
	m.guard.close = closeIntent{}
	m.guard.evict = evictIntent{}
	m.guard.quit = quitIntent{}
	return m
}

// confirmGuardSave transitions the CURRENT guard prompt from guardPrompting
// to guardAwaitingSave: the footer prompt is dismissed (mirrors
// clearGuardPrompt's footer.SetGuard(..., nil) idempotent-clear idiom —
// handleKeyPress's guard-owns-keyboard gate has usually already raced the
// footer itself there one message earlier) but guard.kind is DELIBERATELY
// preserved, not reset to guardNone — the surviving close/evict/quit intent
// (already stamped/left active by the caller) needs guard.kind to keep
// reading guardDirtyClose/guardDirtyEvict/guardDirtyQuit so a LATER guard
// raised on top (e.g. a divergence discovered mid-save, raiseConflictGuard)
// is recognized as a coexisting, not a replacing, guard (critic R1). Only
// the [S]ave response to guardDirtyClose/guardDirtyEvict/guardDirtyQuit calls
// this — every other guard kind resolves synchronously and always calls
// clearGuardPrompt instead (A3).
func (m Model) confirmGuardSave() Model {
	m.footer = m.footer.SetGuard(footer.GuardDirty, nil)
	m.guard.phase = guardAwaitingSave
	return m
}

// prompting reports whether THIS guard machine currently owns the footer's
// modal prompt. Kept in lockstep with footer.InGuard() by two chokepoints:
// raiseGuardPrompt sets phase=guardPrompting the instant it calls
// footer.SetGuard; handleKeyPress's guard-owns-keyboard gate
// (workspace_update_keys.go) resets phase=guardIdle the instant
// footer.Update resolves the prompt — synchronously, in the SAME message the
// keypress arrived in, before the async footer.DataLossGuardResponseMsg the
// resolution's own Cmd carries is ever delivered. Without that second
// chokepoint, phase would trail footer.InGuard() by one message (the same
// gap GUARD-STATE-COH's own doc comment describes for the pending-state
// correlation), which the fuzz invariant footer.InGuard() ==
// m.guard.prompting() (internal/fuzz/ui/workspace) checks after EVERY
// settled message, not just after the continuation lands.
func (g guardState) prompting() bool { return g.phase == guardPrompting }

// guardSpec is the render recipe workspace supplies the footer for one
// guardKind: which footer.GuardKind renders it, and which options it
// accepts. Absorbs the former workspace_guardopts.go's option vars (below)
// into one table keyed by guardKind (A4: that file's sole remaining content
// — actionKind/pendingDataLoss were deleted, not migrated — landed here).
//
// Scope caveat (critic R5): this does NOT close F12 — footer's own
// guardDescriptorFor (labels + key letters, footer.go) remains a SECOND,
// hand-consistent list until the optional A8 merges them. This table
// relocates the workspace-side half of the duplication, it does not
// eliminate the footer-side half.
type guardSpec struct {
	footerKind footer.GuardKind
	options    []footer.GuardOption
}

var guardSpecs = map[guardKind]guardSpec{
	guardTrash:      {footer.GuardTrash, trashGuardOptions},
	guardDirtyClose: {footer.GuardDirty, dataLossGuardOptions},
	guardDirtyEvict: {footer.GuardDirty, dataLossGuardOptions},
	guardDirtyQuit:  {footer.GuardDirty, dataLossGuardOptions},
	guardConflict:   {footer.GuardMerge, guardMergeOptions},
	guardDeleted:    {footer.GuardDeleted, guardDeletedOptions},
	guardRaced:      {footer.GuardRaced, guardRacedOptions},
	guardDegraded:   {footer.GuardDegraded, guardDegradedOptions},
}

// ---- Guard options (A4: absorbed from the former workspace_guardopts.go) ---

// trashGuardOptions drives the trash-file confirmation prompt. Cancel is LAST
// so Escape means Cancel — Escape must never cause data loss (§1.4.4).
var trashGuardOptions = []footer.GuardOption{
	{Key: 'y', Response: footer.DataLossTrash},
	{Key: 0, Response: footer.DataLossCancel}, // Esc → last option = Cancel
}

// dataLossGuardOptions drives the dirty-buffer prompt (close/evict/quit).
// Cancel is LAST so that Escape (which the footer resolves to the final
// option) means Cancel, never Discard — Escape must never lose data (Fix 7 §1).
var dataLossGuardOptions = []footer.GuardOption{
	{Key: 's', Response: footer.DataLossSave},
	{Key: 'd', Response: footer.DataLossDiscard},
	{Key: 0, Response: footer.DataLossCancel}, // Esc → guardOptions[len-1] = Cancel
}

// guardMergeOptions drives the conflict guard prompt ([S]/[D]/[M]/Esc). Cancel
// is LAST so Esc means Cancel (never destructive). Enter is also neutralized to
// Cancel for this guard (R4) — a stray Enter must never pick [S]ave anyway.
var guardMergeOptions = []footer.GuardOption{
	{Key: 's', Response: footer.DataLossSaveAnyway},
	{Key: 'd', Response: footer.DataLossDiscard},
	{Key: 'm', Response: footer.DataLossMerge},
	{Key: 0, Response: footer.DataLossCancel}, // Esc → Cancel
}

// guardDeletedOptions drives the file-deleted-on-disk guard prompt
// ([S]ave/[D]iscard/Esc). Cancel is LAST so Esc means Cancel (never
// destructive, §1.4.4). There is no [M]erge option — there is no "theirs" to
// merge against, only recreate-or-purge.
var guardDeletedOptions = []footer.GuardOption{
	{Key: 's', Response: footer.DataLossSaveAnyway},
	{Key: 'd', Response: footer.DataLossDiscard},
	{Key: 0, Response: footer.DataLossCancel}, // Esc → Cancel
}

// guardRacedOptions drives the swap-race guard prompt (F5: GuardRaced —
// [K]eep mine/[R]estore theirs/Esc). Cancel is LAST so Esc means Cancel
// (§1.4.4) — safe either way here, since our write already committed
// physically; Cancel simply defers the keep-vs-restore decision (equivalent
// to keep-mine for now — nothing is lost, the displaced bytes stay
// recoverable history regardless).
var guardRacedOptions = []footer.GuardOption{
	{Key: 'k', Response: footer.DataLossKeepMine},
	{Key: 'r', Response: footer.DataLossRestoreTheirs},
	{Key: 0, Response: footer.DataLossCancel}, // Esc → Cancel
}

// guardDegradedOptions drives the degraded-storage confirmation prompt
// ([Y]es, save anyway/Esc). Cancel is LAST so Esc means Cancel (§1.4.4) —
// declining never loses anything already typed, it just defers the save.
var guardDegradedOptions = []footer.GuardOption{
	{Key: 'y', Response: footer.DataLossConfirmDegraded},
	{Key: 0, Response: footer.DataLossCancel}, // Esc → Cancel
}

// raiseGuardPrompt is the ONLY sanctioned caller of footer.SetGuard (A2/F12)
// — every data-loss/conflict prompt workspace raises funnels through here,
// so the footer.SetGuard call itself is single-sourced even though the
// (kind, options) PAIRS it draws from are still the option vars above
// (unchanged by this stage). guardNone is a documented no-op — no
// workspace call site raises it; it exists so guardKind's zero value is
// meaningful (§1.1) rather than an unrepresentable "no guard".
func (m Model) raiseGuardPrompt(kind guardKind) Model {
	spec, ok := guardSpecs[kind]
	if !ok {
		return m
	}
	m.footer = m.footer.SetGuard(spec.footerKind, spec.options)
	m.guard.kind = kind
	m.guard.phase = guardPrompting
	return m
}

// clearGuardPrompt dismisses whatever guard prompt is currently showing.
// footer.InGuard() is len(guardOptions) > 0 (footer.go), so SetGuard with a
// nil options slice is the sanctioned clear idiom — the guardKind argument
// is irrelevant once options is nil (SetGuard also resets guardLabel).
// Resets guard.kind to guardNone alongside phase=guardIdle — every guard
// RESOLUTION handler (handleDataLoss*/handleDeleted*) calls this once its
// own intent is fully consumed, so a stale kind can never survive to
// misroute a later, unrelated guard's response (the kind-first dispatcher
// A3's final commit introduces depends on this). Idempotent — safe to call
// when phase is already guardIdle (handleKeyPress's guard-owns-keyboard gate
// already raced it there for the SAME resolution, one message earlier).
func (m Model) clearGuardPrompt() Model {
	m.footer = m.footer.SetGuard(footer.GuardDirty, nil)
	m.guard.kind = guardNone
	m.guard.phase = guardIdle
	return m
}
