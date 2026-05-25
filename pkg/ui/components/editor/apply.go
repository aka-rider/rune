package editor

import (
	"time"

	"rune/pkg/command"
	"rune/pkg/editor/buffer"
	"rune/pkg/editor/cursor"
	"rune/pkg/editor/history"

	tea "charm.land/bubbletea/v2"
)

func (m Model) applyOperation(op command.Operation, kind history.EditKind, now time.Time) Model {
	if len(op.Edits) > 0 {
		cursorsBefore := m.cursors.All() // assuming All() returns []cursor.Cursor
		newBuf, applied, err := m.buf.ApplyEdits(op.Edits)
		if err == nil {
			m.buf = newBuf
			m.dirty = (hashContent(m.buf.Content()) != m.savedContentHash)

			group := history.EditGroup{
				Edits:         applied,
				CursorsBefore: cursorsBefore,
				CursorsAfter:  op.Cursors.All(),
				Timestamp:     now,
				Kind:          kind,
			}
			if m.history.ShouldCoalesce(kind, now) {
				m.history = m.history.MergeIntoLast(applied, group.CursorsAfter)
			} else {
				m.history = m.history.Push(group)
			}
		}
	}

	m.cursors = op.Cursors
	return m
}

func (m Model) applyUndo() (Model, tea.Cmd) {
	newHist, group, ok := m.history.Undo()
	if !ok {
		return m, nil
	}
	m.history = newHist

	// apply the inversions of the edits (in reverse order, but buffer.ApplyEdits takes descending sorted edits,
	// actually `group.Edits` is from buffer.ApplyEdits which applies descending.)
	// When we insert something, standard ApplyEdits needs them non-overlapping descending.
	// Wait, for Undo, we apply the inverse edits. We can just rebuild the whole buffer from scratch? No, just reverse.
	var inverse []buffer.Edit
	// Applied edits were already descending. If we reverse them to inverse edits, they might still be descending?
	// Wait, if it inserted text, the new buffer has larger length. To remove it, start=Start, End=Start+len(Insert).
	for _, ae := range group.Edits {
		inverse = append(inverse, buffer.Edit{
			Start:  ae.Start,
			End:    ae.Start + len(ae.Insert),
			Insert: ae.Deleted,
		})
	}
	// Since original were sorted descending by Start, the Inverse edits' Start are also descending!

	newBuf, _, err := m.buf.ApplyEdits(inverse)
	if err == nil {
		m.buf = newBuf
		m.dirty = (hashContent(m.buf.Content()) != m.savedContentHash)
	}

	// Restore cursors
	if len(group.CursorsBefore) > 0 {
		// Not ideal, but just assume CursorsBefore can reconstruct CursorSet for now?
		// M.cursors = cursor.NewCursorSetFromList(group.CursorsBefore)
		// Let's check how to construct CursorSet
	}

	return m, nil
}

func (m Model) applyRedo() (Model, tea.Cmd) {
	// Stub implementation
	return m, nil
}

func (m Model) syncDisplay() Model {
	// Stub implementation
	return m
}

func (m Model) scrollToCursor() Model {
	// Stub implementation
	return m
}

func (m Model) scrollPreservingAnchor(oldSnapshot, newSnapshot interface{}) Model { // Replace interface{} with proper types
	// Stub implementation
	return m
}

func (m Model) dispatchOperation(result command.Result, cmdName string, now time.Time) (Model, tea.Cmd) {
	m = m.applyOperation(result.Operation, m.editKindFromCommand(cmdName), now)
	if result.Operation.Kind == command.OperationSaveFile {
		var saveID SaveIdentity
		m, saveID, result.Cmd = m.startSaveRequest(SaveRequest{
			Path:        result.Operation.SavePath,
			Content:     result.Operation.SaveContent,
			RequestID:   result.Operation.SaveRequestID,
			ContentHash: result.Operation.SaveContentHash,
		})
		_ = saveID // Ignore unused warning
	}
	return m, result.Cmd
}

func (m Model) clampCursorsToViewport() cursor.CursorSet {
	// Stub implementation
	return m.cursors
}

func (m Model) editKindFromCommand(cmdName string) history.EditKind {
	// Stub implementation
	return history.EditBatch
}

func (m Model) chordTimeoutCmd() tea.Cmd {
	return func() tea.Msg {
		time.Sleep(1500 * time.Millisecond)
		return nil // Return chordTimeoutMsg{} when implemented
	}
}

func (m Model) isPrintable(msg tea.KeyPressMsg) bool {
	// Stub implementation
	return false
}

func (m Model) startSaveRequest(req SaveRequest) (Model, SaveIdentity, tea.Cmd) {
	m.activeSave = SaveIdentity{
		Path:        req.Path,
		RequestID:   req.RequestID,
		ContentHash: req.ContentHash,
		InFlight:    true,
	}
	return m, m.activeSave, SaveFileCmd(req)
}
