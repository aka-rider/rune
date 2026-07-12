package opentabs

// FuzzTabs returns a copy of the current tab slice for invariant checking.
// Active state is derived separately via ActiveHandle(); Dirty is in each Tab.
func (m Model) FuzzTabs() []Tab {
	result := make([]Tab, len(m.tabs))
	copy(result, m.tabs)
	return result
}

// FuzzActivitySeq returns the current monotonic counter. Invariant drivers use
// this to verify that every tab's lastActiveSeq is always ≤ activitySeq.
func (m Model) FuzzActivitySeq() int64 { return m.activitySeq }
