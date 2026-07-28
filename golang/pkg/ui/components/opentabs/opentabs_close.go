package opentabs

// resyncCursorAfterRemoval restores m.cursor to the ACTIVE tab's index after
// a removal has shifted everyone below it down by one. SetActive's own
// idempotency short-circuit (`if m.activeHandle.Equal(h) { return m }`)
// assumes "same identity ⇒ cursor is still correct" — an assumption a
// same-identity removal-elsewhere (evictSaveAck/evictDiscard closing a
// DIFFERENT, non-active victim while the active doc's own tab never
// changes identity) breaks: a bare index clamp (`cursor >= len(tabs)`)
// only catches the active tab having been the LAST one removed, not an
// EARLIER tab shifting everything below it — leaving cursor pointing at
// whatever tab happens to now occupy the old index (EDITOR-TAB-COH:
// found via FuzzHumanSession's eviction-pressure cluster, WP6 session).
// If the active tab itself was the one just removed, activeHandle matches
// nothing and this is a no-op — Close's own bounds-clamp still applies for
// that case (a caller-selected neighbor takes over via a subsequent
// SetActive).
func (m Model) resyncCursorAfterRemoval() Model {
	for i, t := range m.tabs {
		th := TabHandle{DocID: t.DocID, Path: t.Path}
		if th.Equal(m.activeHandle) {
			m.nav.Cursor = i
			return m
		}
	}
	return m
}

// Close removes the tab identified by h. See findTab for lookup semantics;
// no-op if no tab matches.
func (m Model) Close(h TabHandle) Model {
	i := m.findTab(h)
	if i < 0 {
		return m
	}
	m.tabs = append(m.tabs[:i], m.tabs[i+1:]...)
	if m.nav.Cursor >= len(m.tabs) && m.nav.Cursor > 0 {
		m.nav.Cursor = len(m.tabs) - 1
	}
	return m.resyncCursorAfterRemoval().ensureVisible()
}

// RenameFile updates path and display name for the tab matching oldPath. If
// newPath ALREADY belongs to a DIFFERENT tab, that tab is DETACHED (its Path
// cleared to "") rather than left as a stale duplicate (T1) or silently
// clobbered: disk truth wins — newPath's bytes now verifiably belong to the
// document being renamed (store.Load's inode match, §1.4.6), so the file the
// other tab used to track is verifiably gone, exactly as if it had been
// deleted externally. Detaching touches ONLY the Path/Name label — content,
// dirty state, and docstate history are completely untouched (§0/§1.4.4: no
// edit is discarded, no confirmation is skipped, nothing is lost) — the
// detached tab behaves like any other unsaved/untitled tab from here
// (T1 explicitly permits multiple ""-path tabs), including the normal
// bind-new-on-save flow if the user saves it under a different name.
// ok=false signals a collision was reconciled this way, so the caller can
// tell the difference from an ordinary rename (ok=true).
func (m Model) RenameFile(oldPath, newPath string) (Model, bool) {
	idx := -1
	for i, t := range m.tabs {
		if t.Path == oldPath {
			idx = i
			break
		}
	}
	if idx < 0 {
		return m, true // nothing to rename
	}
	detached := false
	// newPath=="" is never a real collision — see OpenFile's detachOther.
	if newPath != "" {
		for i := range m.tabs {
			if i != idx && m.tabs[i].Path == newPath {
				m.tabs[i].Path = ""
				detached = true
			}
		}
	}
	m.tabs[idx].Path = newPath
	m.tabs[idx].Name = tabName(newPath)
	return m, !detached
}
