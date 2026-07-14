package snapshot

import (
	"rune/pkg/ui/components/footer"
	"rune/pkg/ui/components/markdownedit"
	"rune/pkg/ui/components/opentabs"
	"rune/pkg/ui/components/textedit"
)

// FromTextedit builds a partial Snapshot from a standalone textedit.Model —
// exactly one field mapping, shared by workspace_fuzz.go's FuzzInspect (the
// editor-derived fields of its full Snapshot) and
// internal/fuzz/invarianttest's CheckTextedit (a standalone textedit.Model
// under test, with every other Snapshot field left zero-valued). Fields
// outside this mapping (Tabs, GuardVisible, ...) are the caller's to fill in
// if it has that context; the per-domain checkers (textedit's Check) only
// read the fields their own invariants need.
func FromTextedit(m textedit.Model) Snapshot {
	return Snapshot{
		Content:       m.Content(),
		Cells:         m.FuzzCells(),
		CursorOffsets: m.CursorOffsets(),
		Focused:       m.Focused(),
		ReadOnly:      m.ReadOnly(),

		Cursors:       m.FuzzCursors(),
		BufferVersion: m.FuzzBufferVersion(),
		LineCount:     m.FuzzLineCount(),
		LastEdits:     m.FuzzLastEdits(),

		Display: m.FuzzSnapshot(),
		Wrap:    m.FuzzWrapSnapshot(),
		Syntax:  m.FuzzSyntaxSnapshot(),

		EditorWidth: m.FuzzEditorWidth(),
	}
}

// FromMarkdownedit builds a partial Snapshot from a standalone
// markdownedit.Model. markdownedit_fuzz.go's Fuzz* methods are pure forwards
// to the embedded textedit.Model, so this reuses FromTextedit on the
// embedded field directly — one mapping, not a second copy of it.
func FromMarkdownedit(m markdownedit.Model) Snapshot {
	return FromTextedit(m.Model)
}

// FromOpenTabs builds a partial Snapshot from a standalone opentabs.Model —
// exactly one field mapping, shared by workspace_fuzz.go's FuzzInspect (the
// tab-derived fields of its full Snapshot, formerly workspace's private
// tabsInfo helper) and internal/fuzz/invarianttest's CheckOpenTabs.
func FromOpenTabs(m opentabs.Model) Snapshot {
	raw := m.FuzzTabs()
	activeHandle := m.ActiveHandle()
	tabs := make([]TabInfo, len(raw))
	active := make([]bool, len(raw))
	hasDirty := false
	activeTabDirty := false
	for i, t := range raw {
		tabs[i] = TabInfo{Path: t.Path, Name: t.Name, DocID: t.DocID}
		th := opentabs.TabHandle{DocID: t.DocID, Path: t.Path}
		isActive := th.Equal(activeHandle)
		active[i] = isActive
		if t.Dirty {
			hasDirty = true
			if isActive {
				activeTabDirty = true
			}
		}
	}
	return Snapshot{
		Tabs:           tabs,
		ActiveTabIdx:   m.Cursor(),
		TabActive:      active,
		TabCount:       m.Len(),
		HasDirtyFile:   hasDirty,
		ActiveTabDirty: activeTabDirty,
	}
}

// FromFooter builds a partial Snapshot from a standalone footer.Model —
// exactly one field mapping, shared by workspace_fuzz.go's FuzzInspect (the
// footer-derived fields of its full Snapshot) and
// internal/fuzz/invarianttest's CheckFooter.
func FromFooter(m footer.Model) Snapshot {
	return Snapshot{
		GuardVisible:     m.InGuard(),
		GuardKind:        m.GuardKind(),
		GuardOptionCount: m.GuardOptionCount(),
		ChordPending:     m.PendingKey() != "",
		StoreDegraded:    m.Degraded(),
	}
}
