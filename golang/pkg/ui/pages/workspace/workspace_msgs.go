package workspace

import "rune/pkg/docstate"

// ---- Message types ----

// ErrMsg signals a non-fatal I/O error to the workspace.
type ErrMsg struct{ Err error }

// dirChangedMsg signals the watched directory changed on disk.
type dirChangedMsg struct{}

// fileChangedMsg signals fsnotify observed an in-place Write to a file in the
// watched directory (path = the written file). Drives the passive "changed on
// disk" hint for in-place edits the dir-level Create/Remove/Rename events miss
// (BUG1). The handler filters to the current file and ignores our own saves
// (which land via temp→rename → Create, not Write).
type fileChangedMsg struct {
	path string
}

// fileWatchReadError signals fsnotify detected a write but the file could not
// be re-read (deleted, moved, or permission denied).
type fileWatchReadError struct {
	path string
	err  error
}

// StoreReadyMsg is emitted when the docstate store has been opened.
// docstate.Open has no hard-fail outcome anymore (every failure degrades to
// an in-memory fallback with a Warning instead) — a store-open error now
// flows through the generic ErrMsg path instead of this message, so
// StoreReadyMsg always carries a usable Store.
type StoreReadyMsg struct {
	Store   *docstate.Store
	Warning string
}

// AutosaveSettledMsg is emitted after a VFS snapshot goroutine completes.
// Exported so the fuzz driver can detect autosave completion for DL1 checks.
// err is non-nil when the snapshot write failed (surfaced to the user; the
// journal remains the durable record).
type AutosaveSettledMsg struct {
	gen uint64
	err error
}

// pendingFlushMsg is returned by the debounce goroutine. The handler checks
// gen == m.flushGen before firing snapshotCmd so only the latest flush wins.
type pendingFlushMsg struct{ gen uint64 }
