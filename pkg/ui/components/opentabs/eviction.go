package opentabs

// HasTab reports whether a tab is already open for the given identity. When
// docID is non-zero the lookup is by docID (rename-safe); when docID is zero
// the lookup is by path (for virtual docs such as help).
func (m Model) HasTab(docID int64, path string) bool {
	if docID != 0 {
		for _, t := range m.tabs {
			if t.DocID == docID {
				return true
			}
		}
		return false
	}
	for _, t := range m.tabs {
		if t.DocID == 0 && t.Path == path {
			return true
		}
	}
	return false
}

// EvictionCandidate finds the best tab to close when the bar is at capacity.
//
// Eligible tabs must be:
//   - not the active tab
//   - not pinned
//   - bound to a file (DocID != 0 and Path != "")
//
// Among eligible tabs, a clean one is always preferred over a dirty one so
// that silent eviction (no prompt) is lossless. Within each tier the tab with
// the smallest lastActiveSeq (least recently active) is chosen.
//
// Returns (victim, dirty, ok). ok is false when no eligible tab exists.
func (m Model) EvictionCandidate() (TabHandle, bool, bool) {
	var (
		cleanHandle TabHandle
		cleanSeq    int64
		hasClean    bool

		dirtyHandle TabHandle
		dirtySeq    int64
		hasDirty    bool
	)

	for _, t := range m.tabs {
		th := TabHandle{DocID: t.DocID, Path: t.Path}
		if th.Equal(m.activeHandle) || t.Pinned || t.DocID == 0 || t.Path == "" {
			continue // skip: active, pinned, help (DocID==0), or untitled draft
		}
		if t.Dirty {
			if !hasDirty || t.lastActiveSeq < dirtySeq {
				dirtyHandle = TabHandle{DocID: t.DocID, Path: t.Path}
				dirtySeq = t.lastActiveSeq
				hasDirty = true
			}
		} else {
			if !hasClean || t.lastActiveSeq < cleanSeq {
				cleanHandle = TabHandle{DocID: t.DocID, Path: t.Path}
				cleanSeq = t.lastActiveSeq
				hasClean = true
			}
		}
	}

	if hasClean {
		return cleanHandle, false, true
	}
	if hasDirty {
		return dirtyHandle, true, true
	}
	return TabHandle{}, false, false
}
