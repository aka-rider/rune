package workspace

import (
	"rune/pkg/ui/components/opentabs"
)

// groundTruthDirty queries store.DirtyDocs for every open tab that has a
// real VFS docID (untitled tabs are store-backed via CreateScratch/
// ensureScratchDoc, so they're included too — only the help tab, docID==0,
// is excluded structurally) and returns the fresh per-doc result. nil means
// no store, or nothing to query — callers fall back to opentabs' own cached
// flag in that case (it's the only fact available before the store exists).
//
// H3 (§1.4.8): opentabs.Dirty is a per-tab cache, updated at the moment of
// each known transition (an edit journals, a save/materialize commits) — but
// a transition this cache doesn't know about (or simply drifts from) leaves
// it stale. A DESTRUCTIVE decision keyed on "clean" (quit without saving,
// silent tab eviction with no prompt) must never trust that cache; it must
// re-derive from the store, which is exactly what IsDirty/DirtyDocs already
// do on every call (never a cached flag on the docstate side either).
func (m Model) groundTruthDirty() map[int64]bool {
	if m.store == nil {
		return nil
	}
	ids := m.opentabs.AllDocIDs()
	if len(ids) == 0 {
		return nil
	}
	dirty, err := m.store.DirtyDocs(ids)
	if err != nil {
		return nil // fire-and-forget: fall back to the cached-flag path
	}
	return dirty
}

// anyDirty reports whether ANY open tab has unsaved changes, ground-truth
// (groundTruthDirty) when a store is available, falling back to opentabs'
// own cached flag only when it isn't (no docIDs to query yet).
func (m Model) anyDirty() bool {
	fresh := m.groundTruthDirty()
	if fresh == nil {
		return m.opentabs.HasDirty()
	}
	for _, dirty := range fresh {
		if dirty {
			return true
		}
	}
	return false
}

// dirtyHandles returns the TabHandle of every open tab that is dirty,
// ground-truth (groundTruthDirty) when available. saveAllDirtyForQuit
// consumes this instead of opentabs.DirtyTabs() (H3) — a background tab
// whose cached flag drifted clean must still be included.
func (m Model) dirtyHandles() []opentabs.TabHandle {
	fresh := m.groundTruthDirty()
	if fresh == nil {
		return m.opentabs.DirtyTabs()
	}
	var out []opentabs.TabHandle
	for i := 0; i < m.opentabs.Len(); i++ {
		docID := m.opentabs.DocIDAt(i)
		if docID != 0 && fresh[docID] {
			out = append(out, opentabs.TabHandle{DocID: docID, Path: m.opentabs.PathAt(i)})
		}
	}
	return out
}

// isDirtyGroundTruth reports docID's fresh dirty status, ground-truth when a
// store is available. Used by eviction-candidate selection (enforceTabLimit)
// to re-verify a "clean" candidate BEFORE silently closing it with no
// prompt — the candidate's cached flag alone must never authorize a silent,
// unprompted discard (H3 / §1.4.8).
func (m Model) isDirtyGroundTruth(docID int64, cachedDirty bool) bool {
	if m.store == nil || docID == 0 {
		return cachedDirty
	}
	dirty, err := m.store.IsDirty(docID)
	if err != nil {
		return cachedDirty // fire-and-forget: fall back to the cached flag
	}
	return dirty
}
