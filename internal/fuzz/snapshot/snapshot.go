// Package snapshot holds the composite Snapshot type that workspace_fuzz.go
// produces after every settled message, and that each per-domain invariant
// checker consumes. Moving Snapshot out of the invariant package keeps the
// invariant package a minimal leaf (only Violation + Monitor interface) and
// lets checker packages import it without dragging in workspace logic.
package snapshot

import (
	"rune/pkg/editor/cursor"
	"rune/pkg/editor/display"
	"rune/pkg/ui/components/footer"
	"rune/pkg/ui/components/textedit"
)

// TabInfo represents a single tab's identity for invariant checking.
type TabInfo struct {
	Path  string
	Name  string
	DocID int64
}

// Snapshot is a flat read-only value capturing workspace state after each
// settled message. FuzzInspect() on workspace.Model produces this.
type Snapshot struct {
	// Editor state
	Content       string
	Cells         [][]textedit.Cell // from renderCells() — same cells View() renders
	CursorOffsets map[int]bool      // active cursor byte offsets
	Focused       bool              // whether the editor pane has focus
	ReadOnly      bool              // WithReadOnly/SetReadOnly(true) — Help view: renderCells
	// deliberately emits zero Cursor=true cells here ("no caret" — its own
	// doc comment), so CursorOffsets legitimately has no matching cell.

	// Editor structural state (from FuzzCursors / fuzz accessors)
	Cursors       []cursor.Cursor
	BufferVersion uint64
	LineCount     int

	// Display pipeline snapshots (for D-family and WRAP-RT/SPAN-COVER)
	Display display.DisplaySnapshot
	Wrap    display.WrapSnapshot
	Syntax  display.SyntaxSnapshot

	// Tab bar state
	Tabs         []TabInfo
	ActiveTabIdx int
	TabActive    []bool // Tab.Active flags (for TAB-SET)
	TabCount     int    // current number of open tabs
	TabLimit     int    // enforced hard cap (0 = uncapped)

	// SHADOW: set by driver (not by FuzzInspect); empty string = not yet checked
	MirrorContent string

	// CloseFileKeyPressed: set by driver (not by FuzzInspect) when the message was a CloseFile key press.
	CloseFileKeyPressed bool

	// File / persistence
	ActiveFilePath string // m.filePath; empty = Untitled/unsaved file
	EditorPath     string // same as filePath; named for clarity in invariants
	DocID          int64

	// Loading is true while an async file read is in flight (pendingLoad.active).
	// During this window the view holds the save-safe transitional untitled while
	// the active tab already points at the incoming doc — the active handle
	// intentionally LEADS the view by one async hop (executeClose/finalize), so
	// path-coherence invariants (EDITOR-TAB-COH) hold only after the load settles.
	Loading      bool
	FlushGen     uint64
	SaveSnapshot []byte // activeSave.SavedContent — content captured at save-start
	SaveInFlight bool

	// PendingReopenActive is true while a navigation request sits deferred
	// behind an in-flight save of the SAME doc (m.pendingReopen.active,
	// requestOpenPath's savingTarget gate): the active tab already points at
	// the deferred target while the view deliberately stays on the current
	// doc until the save's FileSavedMsg lands and replays the navigation.
	// Like Loading, this is a legitimate window where the active handle leads
	// (here: trails) the view — EDITOR-TAB-COH is exempt while it is set.
	PendingReopenActive bool

	// PendingDataLossKind mirrors workspace.actionKind's iota order (None=0,
	// Close=1, Quit=2, Evict=3, Trash=4) — WHY the dirty-buffer guard was
	// raised, if it was. A save whose response resolves a Close/Quit/Evict
	// guard legitimately swaps the displayed buffer once the save settles
	// (save-then-close/quit/evict); a plain interactive ⌘S (kind==None) must
	// not mutate the buffer it just saved.
	PendingDataLossKind int

	// PendingConflictActive/PendingDeletedActive/PendingRacedActive mirror
	// the out-of-band validity bit (§1.7) on workspace's pendingConflict/
	// pendingDeleted/pendingRaced — WHICH non-dirty guard (if any) is armed.
	// StoreDegraded mirrors m.footer.Degraded() (itself mirroring
	// m.store.Degraded(), fixed at store-open time) — the fact GuardDegraded
	// requires to have been raised at all (workspace_edit.go's
	// startSaveDegradedConfirmed), since GuardDegraded has no dedicated
	// pendingXxx struct of its own.
	PendingConflictActive bool
	PendingDeletedActive  bool
	PendingRacedActive    bool
	StoreDegraded         bool

	// SaveRequestID is the in-flight interactive save's RequestID (empty when
	// none) — activeSave.RequestID. MERGE-GUARD-RAISE/DELETED-GUARD-RAISE
	// correlate a FileSaveErrorMsg to THIS save (not a concurrent quit-batch/
	// evict background save acking the same DocID) the same way production's
	// handleFileSaveErrorMsg does: m.activeSave.RequestID == msg.RequestID.
	SaveRequestID string

	// MergeActive/MergeUnresolved mirror mergemode.IsActive/
	// HasUnresolvedConflicts(m.merge) — whether the 3-way merge resolver is
	// currently engaged, and whether it still has unresolved conflict blocks.
	MergeActive     bool
	MergeUnresolved bool

	// Layout (for L1/L2)
	Frame       string
	Width       int
	Height      int
	EditorWidth int // textedit Model.width — 0 means no width set (unwrapped)

	// Guard / chord / focus
	HasDirtyFile     bool
	ActiveTabDirty   bool // true iff the currently active tab has unsaved changes
	GuardVisible     bool
	GuardKind        footer.GuardKind
	GuardOptionCount int
	ChordPending     bool

	// GuardPrompting mirrors workspace's own guardState.prompting()
	// (m.guard.phase == guardPrompting) — workspace's SEMANTIC record of
	// "a guard I raised currently owns the footer prompt", kept in lockstep
	// with GuardVisible (footer.InGuard()) by workspace's own chokepoints
	// (raiseGuardPrompt / the guard-owns-keyboard gate). GUARD-PHASE-SYNC
	// (internal/fuzz/ui/workspace) asserts GuardVisible == GuardPrompting
	// after every settled message — the two-sided equality the older,
	// one-directional GUARD-STATE-COH correlation could not safely make
	// (see that invariant's own doc comment for the message-delay gap it
	// tolerates for the pending-state payload, which GuardPrompting does not
	// share: phase closes synchronously with the footer, one message earlier
	// than the payload does).
	GuardPrompting bool
	FocusPane      int // 0=tree,1=tabs,2=center,3=title,4=chat,5=search
	AppQuitting    bool

	// Filetree (for FT-BOUNDS)
	FiletreeCursor int
	FiletreeLen    int
}
