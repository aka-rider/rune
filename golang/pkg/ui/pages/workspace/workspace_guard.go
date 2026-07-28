package workspace

import (
	"rune/pkg/docstate"
	"rune/pkg/ui/components/footer"
	"rune/pkg/ui/components/opentabs"
)

// guardKind is workspace's own semantic classification of which guard is
// showing — finer-grained than footer.GuardKind, which selects only what
// PROMPTS (F12/critic R5): guardDirtyClose/guardDirtyEvict/guardDirtyQuit all
// render as the identical GuardDirty prompt (same footer.GuardKind, same
// options), but are three semantically distinct reasons the guard was raised
// — guard.cont.kind (W1, formerly A4's separate close/evict/quit intents)
// tracks which one, independently of guardState.kind (critic R1: kind can
// flip to guardConflict on top of a live close/evict/quit continuation, see
// guardState's own doc comment) — and every SetGuard call site already knows
// WHY it's raising a guard, so naming that reason here (rather than only
// picking a footer.GuardKind) keeps workspace's and footer's taxonomies
// aligned rather than introducing a second one.
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
// currently prompting on the footer; prompt (the trash/conflict/deleted/
// raced payload) and cont (the close/evict/quit continuation slot) are
// independently populated and may outlive kind switching to a DIFFERENT guard
// (e.g. a conflict guard raised mid-close/evict/quit-continuation does not
// erase that continuation's own state — see raiseConflictGuard's callers).
// kind's zero value (guardNone) is meaningful — no raise call site ever sets
// it — so an "unrepresentable no guard" state never has to be faked with a
// sentinel.
//
// A4 note: resolving a conflict/deleted/trash/raced/degraded guard that was
// raised ON TOP OF a live close/evict/quit continuation UNCONDITIONALLY
// clears the continuation too (the dispatcher's hoisted abandon for
// conflict/deleted, the shared Cancel branch) — this ABANDONS the
// continuation rather than resuming it once the OTHER guard is dealt with;
// there is no re-arm anywhere in the codebase. Pinned by
// TestConflictDuringCloseSave_CoexistsThenAbandonsClose
// (workspace_guard_test.go).
type guardState struct {
	kind   guardKind
	phase  guardPhase
	prompt promptPayload // W2: collapses the former trashPath/conflictIntent/deletedIntent/racedIntent fields (A3) into the one slot at most one of them is ever live in — see promptPayload's own doc comment
	cont   continuation  // W1: collapses the former close/evict/quit intent fields (A4) into the one slot at most one of them is ever live in — see continuation's own doc comment
}

// promptPayload is the ONE payload slot for whichever of the
// trash/conflict/deleted/raced guards is CURRENTLY prompting (W2, collapsing
// the four former independently-typed intents — trashPath/conflictIntent/
// deletedIntent/racedIntent, A3) — guard.kind is the discriminant (there is
// no separate "active" bit here, unlike guard.cont): which fields are
// meaningful depends on guard.kind:
//   - guardTrash:    path only.
//   - guardConflict: docID, path, freshObs.
//   - guardDeleted:  docID, path.
//   - guardRaced:    docID, path, saved, fresh.
//
// Safe to collapse into one slot because every raise site (raiseConflictGuard/
// raiseDeletedGuard/raiseRacedGuard/the trash raise in workspace_update.go)
// pairs its OWN payload write with raiseGuardPrompt(kind) in the same
// function — kind and the payload always change together, so a payload left
// over from a DIFFERENT, already-resolved kind was already unreachable dead
// state before W2 (nothing reads guard.prompt without first checking
// guard.kind). clearGuardPrompt zeroes this alongside kind/phase. racedQueue
// (workspace.go) reuses this same type for its per-docID backlog — a
// deferred, not-currently-prompting payload, always shaped like the raced
// case.
type promptPayload struct {
	docID    int64
	path     string
	freshObs docstate.ObsID       // conflict
	saved    docstate.Observation // raced
	fresh    docstate.Observation // raced
}

// contKind discriminates which close/evict/quit continuation guard.cont
// currently holds. contNone (the zero value) means no continuation is
// pending — no raise site ever leaves cont non-zero without also raising a
// non-guardNone guard.kind, so "no continuation" never needs a second
// sentinel.
type contKind int

const (
	contNone contKind = iota
	contClose
	contEvict
	contQuit
)

// continuation carries the state a raised dirty close/evict/quit guard must
// survive across the async Save→FileSavedMsg round-trip (§5.5) — ONE slot
// (W1) replacing the three independently-typed closeIntent/evictIntent/
// quitIntent structs (A4), justified by the verified coexistence rule that
// close/evict/quit never coexist with EACH OTHER (they may each coexist with
// a conflict/deleted/raced/trash/degraded guard raised on top — critic R1 —
// but never with one another: raising any one of them always goes through
// raiseDirtyGuard, which abandons whatever the other two held first). kind
// discriminates which payload fields are meaningful:
//   - contClose: requestID only.
//   - contEvict: victim, pendingOpenPath, requestID.
//   - contQuit:  saveLeft (the multi-tab "Save all" batch countdown).
//
// Zero value (kind==contNone) = no pending continuation. Lives at
// guardState.cont (A4: migrated from the former Model.pendingDataLoss).
type continuation struct {
	kind            contKind
	requestID       string             // close/evict: correlates the background save ack
	victim          opentabs.TabHandle // evict: eviction target
	pendingOpenPath string             // evict: file to open once the victim is dealt with
	saveLeft        int                // quit: outstanding per-tab materialize acks before teardown
}

// owns reports whether requestID is the ack for THIS continuation's own
// in-flight background save of kind k (footer.DataLossSave -> startSave/
// evictSave, which stamps cont.requestID — workspace_edit.go/
// workspace_evict.go). An UNSTAMPED continuation (requestID=="") is a guard
// that some OTHER, unrelated save's ack must never clear — GUARD-STATE-COH: a
// save with no idea a close/evict continuation exists (bind-new,
// restore-theirs) can never accidentally "own" and clear someone else's
// still-pending guard.
func (c continuation) owns(k contKind, requestID string) bool {
	return c.kind == k && c.requestID != "" && requestID == c.requestID
}

// abandonDirtyContinuation wholesale-clears the close/evict/quit
// continuation slot — the A4 equivalent of the pre-A4
// `m.pendingDataLoss = pendingDataLoss{}` reset, which zeroed a SINGLE shared
// field regardless of which of close/evict/quit it currently held; W1
// collapsed the three independent guardState fields this used to clear back
// onto that one shared slot, so every site that used to reach for the old
// wholesale reset (the shared Cancel branch, the conflict/deleted guard
// resolution handlers abandoning a coexisting close/evict/quit continuation
// per critic R1, teardownAndQuit) still gets the exact same "whatever was
// pending is gone now" semantics without having to first ask which one it
// was. Does NOT touch guard.kind/phase — callers that also need the
// prompt/kind reset call clearGuardPrompt separately (mirrors the old code,
// where the single pendingDataLoss reset never touched guard.kind either).
func (m Model) abandonDirtyContinuation() Model {
	m.guard.cont = continuation{}
	return m
}

// raiseDirtyGuard wholesale-replaces the continuation slot with c and raises
// kind's guard prompt — absorbs the abandon-then-set-then-raise prologue the
// three dirty-guard raise sites (requestCloseCurrent, enforceTabLimit,
// ConfirmQuitMsg) used to repeat inline: c must NEVER coexist with whatever
// the slot held before (close/evict/quit never coexist with each other), so
// every raise site abandons first, unconditionally, exactly like the pre-W1
// abandon+overwrite-single-field idiom.
func (m Model) raiseDirtyGuard(kind guardKind, c continuation) Model {
	m = m.abandonDirtyContinuation()
	m.guard.cont = c
	return m.raiseGuardPrompt(kind)
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

// guardOwnsInput reports whether a raised guard prompt currently owns all
// input — the §1.4.4 policy point behind the 3 input gates (A6/F15):
// handleKeyPress's Priority 2.1 gate, TabSelectedMsg's early-return, and
// MouseClickMsg's early-return each used to re-implement
// "m.footer.InGuard()" independently. Naming the question here doesn't
// change any of the three gates' own RESPONSE — they stay separate returns
// (key path still forwards the keypress to the footer's chord/resolve state
// machine; mouse/tab paths still drop the message outright) — that asymmetry
// is intentional; this only collapses the three-way re-implementation of
// WHAT the shared question is.
func (m Model) guardOwnsInput() bool {
	return m.footer.InGuard()
}

// clearGuardPrompt dismisses whatever guard prompt is currently showing.
// footer.InGuard() is len(guardOptions) > 0 (footer.go), so SetGuard with a
// nil options slice is the sanctioned clear idiom — the guardKind argument
// is irrelevant once options is nil (SetGuard also resets guardLabel).
// Resets guard.kind to guardNone alongside phase=guardIdle and wholesale-
// clears guard.prompt (W2 — the dispatcher, handleDataLossGuardResponse,
// calls this once per kind case after capturing whatever payload it needs),
// so a stale kind/payload can never survive to misroute a later, unrelated
// guard's response. Idempotent — safe to call when phase is already
// guardIdle (handleKeyPress's guard-owns-keyboard gate already raced it
// there for the SAME resolution, one message earlier).
func (m Model) clearGuardPrompt() Model {
	m.footer = m.footer.SetGuard(footer.GuardDirty, nil)
	m.guard.kind = guardNone
	m.guard.phase = guardIdle
	m.guard.prompt = promptPayload{}
	return m
}
