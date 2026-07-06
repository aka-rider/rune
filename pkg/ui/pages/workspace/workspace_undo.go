package workspace

import (
	"fmt"

	tea "charm.land/bubbletea/v2"
)

// undoTarget picks which document (and which buffer, by routing token) a
// global Ctrl+Z/Ctrl+Shift+Z should act on (I2: one document = one event
// stream — there is no more per-event surface to dispatch on, only a
// caller-chosen target). Chat has its OWN reserved document and is chosen
// only while paneChat is focused; every other focus (including paneTitle —
// title is never journaled, so there is nothing of its own to undo) targets
// the current view's document. docID==0 means nothing durable to undo (e.g.
// help, or a store-less startup untitled).
func (m Model) undoTarget() (docID int64, target string) {
	if m.focus == paneChat {
		return m.chatDocID, "chat"
	}
	return m.view.DocID(), "main"
}

// handleUndo applies one undo step with journal⇄buffer coherence (§1.4.8): it
// PEEKS the target (without moving the journal), applies the inverse to the
// buffer, and commits the position move ONLY if the buffer edit succeeds. A
// failed reapply (stale/out-of-bounds positions — the buffer and journal already
// diverged) surfaces a §1.3 error and leaves the position unmoved, so the journal
// never ends up ahead of the buffer. A peek error (corrupt event, §1.3) is
// surfaced too — never silently read as "nothing to undo".
func (m Model) handleUndo() (Model, tea.Cmd) {
	if m.store == nil || m.viewingHelp() {
		return m, nil
	}
	docID, target := m.undoTarget()
	if docID == 0 {
		return m, nil
	}
	step, ok, err := m.store.UndoPeek(docID)
	if err != nil {
		return m, errorCmd(fmt.Errorf("undo: %w", err))
	}
	if !ok {
		return m, nil
	}
	// H1: an active dictation session anchors at a fixed byte offset (Enable)
	// that this journal jump can shift or invalidate underneath — disable it
	// before touching any buffer (mirrors the merge-gate precedent in
	// routeDictationEdit).
	var dictCmds []tea.Cmd
	m = m.disableDictationForTransition(&dictCmds)
	var cmd tea.Cmd
	var applyErr error
	switch target {
	case "main":
		m.editor, cmd, applyErr = m.editor.ApplyInverse(step.Edits)
		if applyErr == nil {
			m.editor = m.editor.SetCursors(step.Cursors)
		}
	case "chat":
		m.chat, applyErr = m.chat.ApplyInverse(step.Edits)
		if applyErr == nil {
			m.chat = m.chat.SetCursors(step.Cursors)
		}
	}
	if applyErr != nil {
		return m, errorCmd(fmt.Errorf("undo %s: %w", target, applyErr))
	}
	if cerr := m.store.MoveUndoPos(docID, step.NewPos); cerr != nil {
		return m, errorCmd(fmt.Errorf("undo %s: commit position: %w", target, cerr))
	}
	if target == "main" {
		m = m.bumpEpoch() // Part IV: an undo buffer install invalidates every outstanding view ticket
	}
	var resyncCmd tea.Cmd
	m, resyncCmd = m.resyncMergeIfMain(target, docID)
	switch target {
	case "main":
		m = m.setFocus(paneCenter)
	case "chat":
		m = m.setFocus(paneChat)
	}
	m = m.syncDictationAllowed()
	m = m.syncMergeHint()
	return m, tea.Batch(append(dictCmds, cmd, resyncCmd)...)
}

// handleRedo mirrors handleUndo: peek the redo target, reapply to the buffer, and
// advance the journal position only on a clean apply (§1.4.8). A failed reapply
// surfaces a §1.3 error and leaves the position unmoved.
func (m Model) handleRedo() (Model, tea.Cmd) {
	if m.store == nil || m.viewingHelp() {
		return m, nil
	}
	docID, target := m.undoTarget()
	if docID == 0 {
		return m, nil
	}
	step, ok, err := m.store.RedoPeek(docID)
	if err != nil {
		return m, errorCmd(fmt.Errorf("redo: %w", err))
	}
	if !ok {
		return m, nil
	}
	// H1: see handleUndo — a stale dictation anchor must not survive a redo
	// jump either.
	var dictCmds []tea.Cmd
	m = m.disableDictationForTransition(&dictCmds)
	var cmd tea.Cmd
	var applyErr error
	switch target {
	case "main":
		m.editor, cmd, applyErr = m.editor.Reapply(step.Edits)
		if applyErr == nil {
			m.editor = m.editor.SetCursors(step.Cursors)
		}
	case "chat":
		m.chat, applyErr = m.chat.Reapply(step.Edits)
		if applyErr == nil {
			m.chat = m.chat.SetCursors(step.Cursors)
		}
	}
	if applyErr != nil {
		return m, errorCmd(fmt.Errorf("redo %s: %w", target, applyErr))
	}
	if cerr := m.store.MoveUndoPos(docID, step.NewPos); cerr != nil {
		return m, errorCmd(fmt.Errorf("redo %s: commit position: %w", target, cerr))
	}
	if target == "main" {
		m = m.bumpEpoch() // Part IV: a redo buffer install invalidates every outstanding view ticket
	}
	var resyncCmd tea.Cmd
	m, resyncCmd = m.resyncMergeIfMain(target, docID)
	switch target {
	case "main":
		m = m.setFocus(paneCenter)
	case "chat":
		m = m.setFocus(paneChat)
	}
	m = m.syncDictationAllowed()
	m = m.syncMergeHint()
	return m, tea.Batch(append(dictCmds, cmd, resyncCmd)...)
}
