//go:build fuzzing

// Package filetree contains invariant checkers for the filetree component:
// FT-BOUNDS (cursor is in [0, FiletreeLen) when the tree is non-empty).
package filetree

import (
	"fmt"

	"rune/internal/fuzz/invariant"
	"rune/internal/fuzz/snapshot"
)

// Check runs all L0 filetree invariants against s.
// Returns the first violation, or nil.
func Check(s snapshot.Snapshot) *invariant.Violation {
	// FT-BOUNDS: filetree cursor must be in [0, FiletreeLen) or 0 when empty.
	if s.FiletreeLen > 0 {
		if s.FiletreeCursor < 0 || s.FiletreeCursor >= s.FiletreeLen {
			return &invariant.Violation{
				InvariantID: "FT-BOUNDS",
				Message: fmt.Sprintf(
					"FiletreeCursor %d out of [0, %d)", s.FiletreeCursor, s.FiletreeLen,
				),
			}
		}
	}
	return nil
}
