// Package opentabs contains invariant checkers for the opentabs component:
// T1 (no duplicate tab paths), T2 (active tab index in range), TAB-SET
// (exactly one active tab flag when non-empty).
package opentabs

import (
	"fmt"

	"rune/internal/fuzz/invariant"
	"rune/internal/fuzz/snapshot"
)

func trunc(s string, n int) string { return invariant.Trunc(s, n) }

// Check runs all L0 opentabs invariants against s.
// Returns the first violation, or nil.
func Check(s snapshot.Snapshot) *invariant.Violation {
	// T1: no duplicate tab paths (empty paths are unsaved files — allowed to repeat).
	{
		seen := make(map[string]int, len(s.Tabs)) // path -> first index
		for i, tab := range s.Tabs {
			if tab.Path == "" {
				continue
			}
			if j, dup := seen[tab.Path]; dup {
				return &invariant.Violation{
					InvariantID: "T1",
					Message: fmt.Sprintf(
						"duplicate tab path %q at indices %d and %d",
						trunc(tab.Path, 120), j, i,
					),
				}
			}
			seen[tab.Path] = i
		}
	}

	// T2: active tab index in range.
	{
		n := len(s.Tabs)
		if n > 0 {
			if s.ActiveTabIdx < 0 || s.ActiveTabIdx >= n {
				return &invariant.Violation{
					InvariantID: "T2",
					Message: fmt.Sprintf(
						"ActiveTabIdx %d out of range [0, %d]",
						s.ActiveTabIdx, n-1,
					),
				}
			}
		} else {
			if s.ActiveTabIdx != 0 && s.ActiveTabIdx != -1 {
				return &invariant.Violation{
					InvariantID: "T2",
					Message: fmt.Sprintf(
						"ActiveTabIdx %d with empty tab list (want 0 or -1)",
						s.ActiveTabIdx,
					),
				}
			}
		}
	}

	// TAB-SET: exactly one tab is Active when tabs non-empty; all paths unique.
	{
		activeCount := 0
		for _, a := range s.TabActive {
			if a {
				activeCount++
			}
		}
		if len(s.TabActive) > 0 && activeCount != 1 {
			return &invariant.Violation{
				InvariantID: "TAB-SET",
				Message:     fmt.Sprintf("expected exactly 1 active tab, got %d", activeCount),
			}
		}
	}

	return nil
}
