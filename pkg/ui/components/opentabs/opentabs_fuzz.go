//go:build fuzzing

package opentabs

// FuzzTabs returns a copy of the current tab slice for invariant checking.
// Includes Active and Dirty flags needed by TAB-SET and HasDirtyFile.
func (m Model) FuzzTabs() []Tab {
	result := make([]Tab, len(m.tabs))
	copy(result, m.tabs)
	return result
}
