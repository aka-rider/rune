package workspace

import (
	"charm.land/bubbles/v2/key"
	tea "charm.land/bubbletea/v2"

	"rune/internal/fuzz/snapshot"
	"rune/pkg/ui/pages/workspace/mergemode"
)

// FuzzInspect returns a read-only snapshot of model state for invariant checking.
// Called by the driver after every settled message.
// Value receiver — pure read, no side effects on m.
//
// The editor/tabs/footer-derived fields come from snapshot.FromTextedit
// (via markdownedit's embedded textedit.Model)/FromOpenTabs/FromFooter — the
// SAME builder functions internal/fuzz/invarianttest's CheckTextedit/
// CheckOpenTabs/CheckFooter use for a standalone component under test.
// Exactly one field mapping; the workspace-only fields (file/persistence,
// merge, layout, filetree, ...) are layered on top here, since only the
// full workspace has that context.
func (m Model) FuzzInspect() snapshot.Snapshot {
	s := snapshot.FromMarkdownedit(m.editor)
	tabs := snapshot.FromOpenTabs(m.opentabs)
	guard := snapshot.FromFooter(m.footer)

	s.Tabs = tabs.Tabs
	s.ActiveTabIdx = tabs.ActiveTabIdx // same source (opentabs.Cursor) — keep ONE mapping, the builder.
	s.TabActive = tabs.TabActive
	s.TabCount = tabs.TabCount
	s.TabLimit = tabLimit
	s.HasDirtyFile = tabs.HasDirtyFile
	s.ActiveTabDirty = tabs.ActiveTabDirty

	s.GuardVisible = guard.GuardVisible
	s.GuardKind = guard.GuardKind
	s.GuardOptionCount = guard.GuardOptionCount
	s.ChordPending = guard.ChordPending
	s.GuardPrompting = m.guard.prompting()
	// StoreDegraded is guard.StoreDegraded (m.footer.Degraded()) — assigned
	// below alongside the other file/persistence fields for readability.

	// File / persistence
	s.ActiveFilePath = m.view.Path()
	s.EditorPath = m.view.Path()
	s.DocID = m.view.DocID()
	s.Loading = m.pendingLoad.active
	s.FlushGen = m.flushGen
	s.SaveSnapshot = m.activeSave.SavedContent
	s.SaveInFlight = m.activeSave.InFlight
	s.PendingDataLossKind = m.fuzzLegacyPendingKind()
	s.SaveRequestID = m.activeSave.RequestID

	s.PendingReopenActive = m.pendingReopen.active
	s.PendingConflictActive = m.guard.conflict.active
	s.PendingDeletedActive = m.guard.deleted.active
	s.PendingRacedActive = m.guard.raced.active
	s.StoreDegraded = guard.StoreDegraded

	s.MergeActive = mergemode.IsActive(m.merge)
	s.MergeUnresolved = mergemode.HasUnresolvedConflicts(m.merge)

	// Layout — Frame is set by driver; driver also sets Width/Height
	s.Width = m.totalWidth
	s.Height = m.totalHeight

	// Guard / chord / focus
	s.FocusPane = int(m.focus)

	// Filetree
	s.FiletreeCursor = m.filetree.FuzzCursor()
	s.FiletreeLen = m.filetree.FuzzLen()

	return s
}

// fuzzLegacyPendingKind maps current guard/intent state onto the legacy
// actionKind iota values snapshot.Snapshot.PendingDataLossKind has always
// reported (None=0, Close=1, Quit=2, Evict=3, Trash=4) — pinned by
// TestFuzzLegacyPendingKind_LegacyIotaOrder and
// TestFuzzLegacyPendingKind_Trash (workspace_fuzz_test.go) and consumed by
// internal/fuzz/ui/workspace's pendingKind* constants and historical fuzz
// corpora. Trash is tied to guard.kind directly (A3 — trash never survives an
// async Save round-trip, so it carries no separate intent struct); Close/
// Quit/Evict are tied to their OWN intent's .active bit (A4), independently
// of guard.kind — critic R1's coexistence window means guard.kind can read
// guardConflict while guard.close/evict/quit.active is still true (a
// conflict guard raised mid-close/evict/quit-save), and the legacy snapshot
// must keep reporting the close/evict/quit intent in that window exactly as
// the pre-A4 pendingDataLoss.kind (independent of guard.kind by
// construction) always did.
func (m Model) fuzzLegacyPendingKind() int {
	switch {
	case m.guard.kind == guardTrash:
		return 4
	case m.guard.close.active:
		return 1
	case m.guard.quit.active:
		return 2
	case m.guard.evict.active:
		return 3
	}
	return 0
}

// IsCloseFileMsg reports whether msg is a CloseFile (^w) key press.
// Used by the fuzz driver to annotate snapshots for the G3 invariant.
func (m Model) IsCloseFileMsg(msg tea.Msg) bool {
	kp, ok := msg.(tea.KeyPressMsg)
	if !ok {
		return false
	}
	return key.Matches(kp, m.keys.CloseFile)
}

// IsUndoRedoMsg reports whether msg is an Undo (⌘Z) or Redo (^Y) key press —
// the same key.Matches checks handleKeyPress's Priority 2.5 branch uses.
// REDO-CLEAR (WP2) excludes these from "a buffer-changing key press must
// truncate the redo future": undo/redo themselves move the journal position,
// they don't append a fresh edit that abandons one.
func (m Model) IsUndoRedoMsg(msg tea.Msg) bool {
	kp, ok := msg.(tea.KeyPressMsg)
	if !ok {
		return false
	}
	return key.Matches(kp, m.keys.Undo) || key.Matches(kp, m.keys.Redo)
}
