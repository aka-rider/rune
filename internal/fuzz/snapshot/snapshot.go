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
	Path string
	Name string
}

// Snapshot is a flat read-only value capturing workspace state after each
// settled message. FuzzInspect() on workspace.Model produces this.
type Snapshot struct {
	// Editor state
	Content       string
	Cells         [][]textedit.Cell // from renderCells() — same cells View() renders
	CursorOffsets map[int]bool      // active cursor byte offsets
	Focused       bool              // whether the editor pane has focus

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

	// ActiveFileExternallyModified: set by RunHuman when KindExternalWrite fires
	// for the currently active file. Used by the EXT-NOCLOBBER monitor.
	ActiveFileExternallyModified bool

	// File / persistence
	ActiveFilePath string // m.filePath; empty = Untitled/unsaved file
	EditorPath     string // same as filePath; named for clarity in invariants
	DocID          int64

	// Loading is true while an async file read is in flight (pendingLoad.active).
	// During this window the view holds the save-safe transitional untitled while
	// the active tab already points at the incoming doc — the active handle
	// intentionally LEADS the view by one async hop (executeClose/finalize), so
	// path-coherence invariants (EDITOR-TAB-COH) hold only after the load settles.
	Loading bool
	FlushGen       uint64
	SaveSnapshot   []byte // activeSave.SavedContent — content captured at save-start
	SaveInFlight   bool

	// Layout (for L1/L2)
	Frame  string
	Width  int
	Height int

	// Guard / chord / focus
	HasDirtyFile     bool
	ActiveTabDirty   bool // true iff the currently active tab has unsaved changes
	GuardVisible     bool
	GuardKind        footer.GuardKind
	GuardOptionCount int
	ChordPending     bool
	FocusPane        int // 0=tree,1=tabs,2=center,3=title,4=chat,5=search
	AppQuitting      bool

	// Filetree (for FT-BOUNDS)
	FiletreeCursor int
	FiletreeLen    int
}
