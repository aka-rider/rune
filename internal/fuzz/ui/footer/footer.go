//go:build fuzzing

// Package footer contains invariant checkers for the footer component:
// GUARD-SYNC (GuardVisible ⟺ GuardOptionCount > 0) and the G2 transition
// (DataLossGuardResponseMsg must clear the guard).
package footer

import (
	"fmt"

	"rune/internal/fuzz/invariant"
	"rune/internal/fuzz/snapshot"
)

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
	typeName := fmt.Sprintf("%T", msg)

	// G2: DataLossGuardResponseMsg must clear the guard.
	if typeName == "footer.DataLossGuardResponseMsg" && next.GuardVisible {
		vs = append(vs, invariant.Violation{
			InvariantID: "G2",
			Message:     "GuardVisible still true after DataLossGuardResponseMsg",
		})
	}

	_ = prev // reserved for future footer transition invariants
	return vs
}
