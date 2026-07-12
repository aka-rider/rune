package workspace

import (
	"fmt"
	"path/filepath"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/ui/components/footer"
)

// deletedIntent carries the docID/path of the current document whose file
// went missing on disk (deleted, or its parent dir removed), for the
// GuardDeleted footer prompt's [S]ave/[D]iscard responses. active is the
// out-of-band validity bit (§1.7) — docID/path are meaningful only while it is
// true. Cleared on every guard resolution (Save/Discard/Cancel). Lives at
// guardState.deleted (A3; absorbed here from the former
// workspace_guardopts.go by A4).
type deletedIntent struct {
	active bool
	path   string
	docID  int64
}

// handleDeletedSave handles the [S]ave response for the GuardDeleted prompt —
// the open file went missing on disk and the user chose to recreate it.
// Recreates the parent directory first (Materialize's overwrite branch does
// not mkdir — only its create branch does; needed here because the parent dir
// itself may have been removed too), then force-writes the LIVE editor buffer
// via store.Materialize — mirroring handleDataLossSaveAnyway's force-write,
// but there is no "theirs" observation to CAS against (the file is simply
// gone; expect=0 is safe because Materialize's create path never consults
// it). The existing FileSavedMsg active-save path marks the tab clean.
func (m Model) handleDeletedSave() (Model, tea.Cmd) {
	if !m.guard.deleted.active || m.store == nil {
		m = m.abandonDirtyContinuation()
		m.guard.deleted = deletedIntent{}
		return m, nil
	}
	pd := m.guard.deleted
	m.guard.deleted = deletedIntent{}
	m = m.abandonDirtyContinuation()

	if dir := filepath.Dir(pd.path); dir != "" {
		if err := m.fsys().MkdirAll(dir, 0o755); err != nil {
			// Can't recreate the parent dir — surface the error, restore
			// guard.deleted, and re-arm the guard so the user can retry
			// (§1.3; the buffer is untouched). Re-arming (not just restoring
			// the pending state) keeps InGuard() and guard.deleted.active in
			// sync — an error banner alone would leave the guard invisible
			// while still silently blocking re-detection.
			m.guard.deleted = pd
			m = m.raiseGuardPrompt(guardDeleted)
			var cmd tea.Cmd
			m.footer, cmd = m.footer.Update(footer.ShowErrorMsg{
				Text: fmt.Errorf("recreate %q: mkdir %q: %w", pd.path, dir, err).Error(),
			})
			return m, cmd
		}
	}

	// Live buffer, not a stale snapshot — mirrors handleDataLossSaveAnyway:
	// async edits (dictation, etc.) can advance the buffer while the guard is
	// up, and writing anything else would silently drop them (§0).
	liveContent := m.editor.Content()
	seq := m.currentSeqFor(pd.docID)
	requestID := fmt.Sprintf("force-save-deleted-%d", pd.docID)
	m.activeSave = SaveIdentity{RequestID: requestID, SavedContent: []byte(liveContent), InFlight: true, Path: pd.path, DocID: pd.docID}
	// bindNew=true: the user's explicit, prompt-confirmed [S]ave response to
	// GuardDeleted IS the "OK to (re)create a missing target" confirmation
	// (§1.4.4) — mirrors bind-new's own create-allowed intent.
	return m, materializeStoreCmd(m.store, pd.docID, pd.path, liveContent, 0, seq, requestID, true)
}

// handleDeletedDiscard handles the [D]iscard response for the GuardDeleted
// prompt: the user's explicit, prompt-confirmed choice (§1.4.4) to purge the
// doc's VFS history and close the tab rather than recreate the missing file.
// Distinct from the conflict guard's [D]iscard (handleDataLossDiscardConflict,
// which loads theirs) and the dirty-guard discard (which keeps history) — here
// there is nothing to fall back to, so the doc is fully purged.
func (m Model) handleDeletedDiscard() (Model, tea.Cmd) {
	if !m.guard.deleted.active {
		m = m.abandonDirtyContinuation()
		return m, nil
	}
	pd := m.guard.deleted
	m.guard.deleted = deletedIntent{}
	m = m.abandonDirtyContinuation()

	if m.store != nil && pd.docID != 0 {
		if err := m.store.DeleteDoc(pd.docID); err != nil {
			_ = err // fire-and-forget (§1.3): best-effort purge; the tab still closes below
		}
	}
	return m.executeClose(pd.docID, pd.path)
}
