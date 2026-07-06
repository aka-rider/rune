package workspace

import (
	"rune/pkg/ui/components/footer"
	"rune/pkg/ui/components/opentabs"
)

// ---- Data-loss action disambiguation ----

// actionKind records WHY the dirty-buffer guard was raised, so the guard
// response (and the async Save round-trip) knows whether to close this tab,
// quit, or do nothing.
type actionKind int

const (
	actionNone  actionKind = iota
	actionClose            // raised by requestCloseCurrent (^w)
	actionQuit             // raised by ConfirmQuitMsg (^C^C)
	actionEvict            // raised when a dirty tab must be evicted to open a new file
	actionTrash            // raised when the user requests to trash a file (⌦/⌘⌫)
)

// pendingDataLoss carries the state a raised dirty guard must survive across the
// async Save→FileSavedMsg round-trip (§5.5). For actionQuit "Save", saveLeft
// counts the outstanding per-tab materialize acks before teardown; the first
// failure clears the whole action so every buffer is kept. For actionEvict,
// victim identifies the tab to close and pendingOpenPath is the file to open
// once the victim is dealt with; requestID correlates the background save ack.
type pendingDataLoss struct {
	kind             actionKind
	saveLeft         int
	victim           opentabs.TabHandle // eviction target (actionEvict)
	pendingOpenPath  string             // file to open after eviction (actionEvict)
	requestID        string             // correlates evict-save ack (actionEvict + Save)
	pendingTrashPath string             // path to trash after guard confirmation (actionTrash)
}

// pendingDeleted carries the docID/path of the current document whose file
// went missing on disk (deleted, or its parent dir removed), for the
// GuardDeleted footer prompt's [S]ave/[D]iscard responses. active is the
// out-of-band validity bit (§1.7) — docID/path are meaningful only while it is
// true. Cleared on every guard resolution (Save/Discard/Cancel).
type pendingDeleted struct {
	active bool
	path   string
	docID  int64
}

// ---- Guard options ----

// trashGuardOptions drives the trash-file confirmation prompt. Cancel is LAST
// so Escape means Cancel — Escape must never cause data loss (§1.4.4).
var trashGuardOptions = []footer.GuardOption{
	{Key: 'y', Response: footer.DataLossTrash},
	{Key: 0, Response: footer.DataLossCancel}, // Esc → last option = Cancel
}

// dataLossGuardOptions drives the dirty-buffer prompt. Cancel is LAST so that
// Escape (which the footer resolves to the final option) means Cancel, never
// Discard — Escape must never lose data (Fix 7 §1).
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
