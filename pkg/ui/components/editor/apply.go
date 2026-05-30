package editor

import (
	"time"

	"rune/pkg/command"
	"rune/pkg/editor/buffer"
	"rune/pkg/editor/cursor"
	"rune/pkg/editor/display"
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
			coalesce := m.history.ShouldCoalesce(kind, now)
			if coalesce && kind == history.EditInsertChar && isWhitespaceEdit(op.Edits) {
				coalesce = false
			}
			if coalesce {
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

	// Build inverse edits: each applied edit's Insert becomes the range to delete,
	// and its Deleted becomes what we re-insert.
	inverse := make([]buffer.Edit, len(group.Edits))
	for i, ae := range group.Edits {
		inverse[i] = buffer.Edit{
			Start:  ae.Start,
			End:    ae.Start + len(ae.Insert),
			Insert: ae.Deleted,
		}
	}

	// Coalesced groups may have edits in ascending order; ApplyEdits requires descending.
	inverse = buffer.CloneAndSortEditsDescending(inverse)

	newBuf, _, err := m.buf.ApplyEdits(inverse)
	if err == nil {
		m.buf = newBuf
		m.dirty = (hashContent(m.buf.Content()) != m.savedContentHash)
	}

	if len(group.CursorsBefore) > 0 {
		m.cursors = cursor.NewCursorSetFrom(group.CursorsBefore)
	}

	return m, nil
}

func (m Model) applyRedo() (Model, tea.Cmd) {
	newHist, group, ok := m.history.Redo()
	if !ok {
		return m, nil
	}
	m.history = newHist

	// To redo: reconstruct the original edits from AppliedEdits.
	// AppliedEdits record the post-edit state. To redo from the pre-edit buffer,
	// we need to reverse the inverse: delete what was previously deleted, insert what was inserted.
	// The original edits applied to the pre-undo buffer can be reconstructed:
	// For each applied edit (in the post-edit buffer): to get back to post-edit from pre-edit,
	// we need edits that delete the Deleted text and insert the Insert text.
	// The pre-edit positions correspond to Start adjusted for cumulative shifts.
	//
	// Since applied edits are stored descending by Start in post-edit buffer,
	// we reconstruct original edits that are also descending.
	edits := make([]buffer.Edit, len(group.Edits))
	cumulativeShift := 0
	// Process from last (smallest Start) to first (largest Start) to compute shifts
	for i := len(group.Edits) - 1; i >= 0; i-- {
		ae := group.Edits[i]
		originalStart := ae.Start - cumulativeShift
		edits[i] = buffer.Edit{
			Start:  originalStart,
			End:    originalStart + len(ae.Deleted),
			Insert: ae.Insert,
		}
		cumulativeShift += len(ae.Insert) - len(ae.Deleted)
	}

	newBuf, _, err := m.buf.ApplyEdits(edits)
	if err == nil {
		m.buf = newBuf
		m.dirty = (hashContent(m.buf.Content()) != m.savedContentHash)
	}

	if len(group.CursorsAfter) > 0 {
		m.cursors = cursor.NewCursorSetFrom(group.CursorsAfter)
	}

	return m, nil
}

func (m Model) syncDisplay() Model {
	if m.syntaxMap == (display.SyntaxMap{}) {
		m.syntaxMap = display.NewSyntaxMap()
	}
	if m.wrapMap == (display.WrapMap{}) {
		m.wrapMap = display.NewWrapMap(0)
	}

	m.syntaxMap, m.syntaxSnap = m.syntaxMap.Sync(m.buf, m.cursors)
	width := m.width
	if width <= 0 {
		width = 0
	}
	m.wrapMap = m.wrapMap.SetWidth(width)
	m.wrapSnap = m.wrapMap.Sync(m.syntaxSnap)
	m.snapshot = display.BuildSnapshot(m.wrapSnap)
	m.snapshot = display.ExpandImageRows(m.snapshot, m.imageDimsFor)
	return m
}

func (m Model) scrollToCursor() Model {
	if len(m.cursors.All()) == 0 {
		return m
	}
	primary := m.cursors.Primary()
	bp := m.buf.OffsetToLineCol(primary.Position)
	sp := m.syntaxSnap.BufferToSyntax(bp)
	wp := m.wrapSnap.SyntaxToWrap(sp)

	contentH := m.contentHeight()

	// Reconcile wrap-row space (wp.Row) with expanded display-row space.
	// View() slices m.snapshot, whose rows include image-reserved rows that do
	// not exist in m.wrapSnap. Map the cursor's wrap row to its expanded display
	// row by anchoring on its model line's first row in each space and keeping
	// the wrap-row offset within the line (0 for single-row/image lines).
	modelLine := sp.Line
	wrapOffsetWithinLine := wp.Row - m.wrapSnap.ModelLineToFirstRow(modelLine)
	if wrapOffsetWithinLine < 0 {
		wrapOffsetWithinLine = 0
	}
	cursorDisplayRow := m.snapshot.ModelLineToFirstRow(modelLine) + wrapOffsetWithinLine

	// Vertical scroll
	if cursorDisplayRow < m.viewport.TopRow {
		m.viewport.TopRow = cursorDisplayRow
	} else if cursorDisplayRow >= m.viewport.TopRow+contentH {
		m.viewport.TopRow = cursorDisplayRow - contentH + 1
	}

	// Horizontal scroll (only when not soft-wrapping)
	if !m.softWrap {
		if wp.Col < m.viewport.ScrollCol {
			m.viewport.ScrollCol = wp.Col
		} else if wp.Col >= m.viewport.ScrollCol+m.width {
			m.viewport.ScrollCol = wp.Col - m.width + 1
		}
	}

	return m
}

func (m Model) scrollPreservingAnchor(oldSnapshot, newSnapshot interface{}) Model { // Replace interface{} with proper types
	// Stub implementation
	return m
}

func (m Model) dispatchOperation(result command.Result, cmdName string, now time.Time) (Model, tea.Cmd) {
	if result.Operation.Kind == command.OperationHistory {
		switch cmdName {
		case "history.undo":
			m, _ = m.applyUndo()
			m = m.syncDisplay()
			m = m.scrollToCursor()
		case "history.redo":
			m, _ = m.applyRedo()
			m = m.syncDisplay()
			m = m.scrollToCursor()
		}
		var dcmd tea.Cmd
		m, dcmd = m.discoverNewImages()
		return m, tea.Batch(result.Cmd, dcmd)
	}

	// Scroll operations adjust viewport without editing.
	if result.Operation.Kind == command.OperationScroll {
		m.viewport.TopRow += result.Operation.ScrollDY
		m.viewport.ScrollCol += result.Operation.ScrollDX
		if m.viewport.TopRow < 0 {
			m.viewport.TopRow = 0
		}
		maxTop := m.snapshot.TotalRows - m.contentHeight()
		if maxTop < 0 {
			maxTop = 0
		}
		if m.viewport.TopRow > maxTop {
			m.viewport.TopRow = maxTop
		}
		if m.viewport.ScrollCol < 0 {
			m.viewport.ScrollCol = 0
		}
		// Resume frame animation for any images scrolled back into view.
		var acmd tea.Cmd
		m, acmd = m.armImageTicks()
		// Re-place iTerm2 images at new screen positions.
		var icmd tea.Cmd
		m, icmd = m.replotInlineImages()
		return m, tea.Batch(result.Cmd, acmd, icmd)
	}

	// OperationNone with a Cmd: clipboard copy/paste just propagate the Cmd.
	if result.Operation.Kind == command.OperationNone {
		m.cursors = result.Operation.Cursors
		return m, result.Cmd
	}

	m = m.applyOperation(result.Operation, m.editKindFromCommand(cmdName), now)
	m = m.syncDisplay()
	m = m.scrollToCursor()

	// Detect image collapse (cursor entered an image tag) and request a full
	// screen repaint to clear ghost iTerm2/WezTerm pixels that were placed
	// out-of-band and are invisible to Bubble Tea's renderer.
	var collapsedCmd tea.Cmd
	var collapsed bool
	m, collapsed = m.detectImageCollapse()
	if collapsed {
		collapsedCmd = tea.ClearScreen
	}

	if result.Operation.Kind == command.OperationSaveFile {
		var saveID SaveIdentity
		m, saveID, result.Cmd = m.startSaveRequest(SaveRequest{
			Path:        result.Operation.SavePath,
			Content:     result.Operation.SaveContent,
			RequestID:   result.Operation.SaveRequestID,
			ContentHash: result.Operation.SaveContentHash,
		})
		_ = saveID
	}

	var dcmd tea.Cmd
	m, dcmd = m.discoverNewImages()
	return m, tea.Batch(result.Cmd, dcmd, collapsedCmd)
}

func (m Model) clampCursorsToViewport() cursor.CursorSet {
	// Stub implementation
	return m.cursors
}

func (m Model) editKindFromCommand(cmdName string) history.EditKind {
	switch cmdName {
	case "edit.insert-character":
		return history.EditInsertChar
	case "edit.delete-left", "edit.delete-right":
		return history.EditDeleteChar
	case "edit.newline":
		return history.EditNewline
	case "clipboard.paste":
		return history.EditPaste
	case "edit.move-line-up", "edit.move-line-down":
		return history.EditMoveLine
	case "edit.clone-line-up", "edit.clone-line-down":
		return history.EditCloneLine
	default:
		return history.EditBatch
	}
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

func isWhitespaceEdit(edits []buffer.Edit) bool {
	for _, e := range edits {
		if e.Insert == " " || e.Insert == "\t" || e.Insert == "\n" {
			return true
		}
	}
	return false
}
