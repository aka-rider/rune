// Package footer contains invariant checkers for the footer component:
// GUARD-SYNC (GuardVisible ⟺ GuardOptionCount > 0) and the G2 transition
// (DataLossGuardResponseMsg must clear the guard).
package footer

import (
	"fmt"

	"rune/internal/fuzz/invariant"
	"rune/internal/fuzz/snapshot"
	uifooter "rune/pkg/ui/components/footer"
)

// reRaisesOnDivergence reports whether response r's handler can legitimately
// re-arm a guard SYNCHRONOUSLY, within the same Update call that processes
// footer.DataLossGuardResponseMsg — so G2 must not treat next.GuardVisible
// as a bug for these:
//   - Save/SaveAnyway/ConfirmDegraded all funnel through startSave's (or
//     startSaveDegradedConfirmed's) §1.4.8 pre-write re-check
//     (workspace_edit.go): it re-derives divergence FRESH at save-time —
//     deliberately, so an earlier Esc-dismissed conflict can never let a
//     later [S]ave silently clobber theirs — and re-raises GuardMerge the
//     instant it finds SyncDiverged again.
//   - RestoreTheirs (the Raced guard's [R]) re-arms GuardRaced on a blob
//     read failure or a failed journal append (workspace_raced.go), so the
//     choice survives a retryable error instead of being silently lost.
//
// Discard/Cancel/MergeAccept/MergeReject/Trash/KeepMine never re-raise
// synchronously (Discard/Merge for the conflict guard clear the guard and
// launch an ASYNC fresh probe — the guard only reappears later, from a
// DIFFERENT message, which G2 correctly still catches).
//
// The message and its Response constants are used directly from the
// production footer package (aliased uifooter — this checker package is
// itself named footer): DataLossGuardResponseMsg and DataLossGuardResponse
// are exported, so no %T/reflect indirection or hand-mirrored iota copy is
// needed — a mirror here could silently drift if the production const block
// were ever reordered.
func reRaisesOnDivergence(r uifooter.DataLossGuardResponse) bool {
	switch r {
	case uifooter.DataLossSave, uifooter.DataLossSaveAnyway,
		uifooter.DataLossConfirmDegraded, uifooter.DataLossRestoreTheirs:
		return true
	default:
		return false
	}
}

// Check runs all L0 footer invariants against s.
// Returns the first violation, or nil.
func Check(s snapshot.Snapshot) *invariant.Violation {
	// GUARD-SYNC: GuardOptionCount > 0 ⟺ GuardVisible; both clear atomically.
	if s.GuardVisible != (s.GuardOptionCount > 0) {
		return &invariant.Violation{
			InvariantID: "GUARD-SYNC",
			Message: fmt.Sprintf(
				"GuardVisible=%v but GuardOptionCount=%d (must agree)",
				s.GuardVisible, s.GuardOptionCount,
			),
		}
	}
	return nil
}

// CheckTransition runs footer-domain L1 transition invariants.
// Returns all violations found.
func CheckTransition(prev snapshot.Snapshot, msg any, next snapshot.Snapshot) []invariant.Violation {
	var vs []invariant.Violation

	// G2: DataLossGuardResponseMsg must clear the guard — EXCEPT a response
	// whose handler can legitimately re-arm one synchronously (see
	// reRaisesOnDivergence): a save-triggering response's fresh §1.4.8
	// divergence re-check finding SyncDiverged again is the safety feature
	// working as designed, not a stuck guard.
	if resp, ok := msg.(uifooter.DataLossGuardResponseMsg); ok && next.GuardVisible {
		if !reRaisesOnDivergence(resp.Response) {
			vs = append(vs, invariant.Violation{
				InvariantID: "G2",
				Message:     "GuardVisible still true after DataLossGuardResponseMsg",
			})
		}
	}

	_ = prev // reserved for future footer transition invariants
	return vs
}
