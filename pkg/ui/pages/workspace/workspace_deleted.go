package workspace

import (
	"fmt"
	"path/filepath"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/ui/components/footer"
)

// handleDeletedSave handles the [S]ave response for the GuardDeleted prompt —
// the open file went missing on disk and the user chose to recreate it.
// Recreates the parent directory first (Materialize's overwrite branch does
// not mkdir — only its create branch does; needed here because the parent dir
// itself may have been removed too), then force-writes the LIVE editor buffer
// via store.Materialize — mirroring handleDataLossSaveAnyway's force-write,
// but there is no "theirs" observation to CAS against (the file is simply
// gone; expect=0 is safe because Materialize's create path never consults
// it). The existing FileSavedMsg active-save path marks the tab clean.
//
// W2: pure consumer of the promptPayload the dispatcher
// (handleDataLossGuardResponse) already captured from guard.prompt, cleared,
// and hoisted the shared abandonDirtyContinuation call for — no more
// "if !active" prologue (the dispatcher only calls this from the
// guardDeleted case).
func (m Model) handleDeletedSave(p promptPayload) (Model, tea.Cmd) {
	if m.store == nil {
		return m, nil
	}

	if dir := filepath.Dir(p.path); dir != "" {
		if err := m.fsys().MkdirAll(dir, 0o755); err != nil {
			// Can't recreate the parent dir — surface the error, restore
			// guard.prompt, and re-arm the guard so the user can retry
			// (§1.3; the buffer is untouched). Re-arming (not just restoring
			// the pending state) keeps InGuard() in sync — an error banner
			// alone would leave the guard invisible while still silently
			// blocking re-detection.
			m.guard.prompt = p
			m = m.raiseGuardPrompt(guardDeleted)
			var cmd tea.Cmd
			m.footer, cmd = m.footer.Update(footer.ShowErrorMsg{
				Text: fmt.Errorf("recreate %q: mkdir %q: %w", p.path, dir, err).Error(),
			})
			return m, cmd
		}
	}

	// Live buffer, not a stale snapshot — mirrors handleDataLossSaveAnyway:
	// async edits (dictation, etc.) can advance the buffer while the guard is
	// up, and writing anything else would silently drop them (§0).
	liveContent := m.editor.Content()
	var cmd tea.Cmd
	// bindNew=true: the user's explicit, prompt-confirmed [S]ave response to
	// GuardDeleted IS the "OK to (re)create a missing target" confirmation
	// (§1.4.4) — mirrors bind-new's own create-allowed intent.
	m, _, cmd = m.issueSave(saveReq{prefix: "force-save-deleted", docID: p.docID, path: p.path, content: liveContent, bindNew: true, track: true})
	return m, cmd
}

// handleDeletedDiscard handles the [D]iscard response for the GuardDeleted
// prompt: the user's explicit, prompt-confirmed choice (§1.4.4) to purge the
// doc's VFS history and close the tab rather than recreate the missing file.
// Distinct from the conflict guard's [D]iscard (handleDataLossDiscardConflict,
// which loads theirs) and the dirty-guard discard (which keeps history) — here
// there is nothing to fall back to, so the doc is fully purged.
//
// W2: pure consumer of the promptPayload — see handleDeletedSave's doc
// comment.
func (m Model) handleDeletedDiscard(p promptPayload) (Model, tea.Cmd) {
	if m.store != nil && p.docID != 0 {
		if err := m.store.DeleteDoc(p.docID); err != nil {
			_ = err // fire-and-forget (§1.3): best-effort purge; the tab still closes below
		}
	}
	return m.executeClose(p.docID, p.path)
}
