package workspace

import (
	"fmt"
	"time"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/docstate"
	"rune/pkg/editor/buffer"
	"rune/pkg/editor/cursor"
	"rune/pkg/ui/components/footer"
)

func (m Model) startSave() (Model, tea.Cmd) {
	if m.filePath == "" || m.activeSave.InFlight {
		return m, nil
	}
	content := m.editor.Content()
	requestID := fmt.Sprintf("save-%v", time.Now().UnixNano())
	m.activeSave = SaveIdentity{
		RequestID:    requestID,
		SavedContent: []byte(content),
		InFlight:     true,
	}
	// Overwrite an already-bound file: materializeCmd refuses if it diverged from
	// our baseline since load (§1.4.7).
	return m, materializeCmd(m.docID, m.filePath, content, requestID, false, m.baseline)
}

// effectiveJournalPos maps the undo pointer and max journal seq to a single
// integer that uniquely identifies a content state. undoSeq is -1 when the
// pointer is at head (mirrors SQL NULL); storeMaxSeq is the highest seq ever
// written for this document (AUTOINCREMENT — never reused after deletion).
func effectiveJournalPos(undoSeq, storeMaxSeq int64) int64 {
	if undoSeq < 0 || undoSeq >= storeMaxSeq {
		return storeMaxSeq
	}
	return undoSeq
}

func (m Model) syncDirty() Model {
	if m.viewingHelp() {
		return m
	}
	dirty := effectiveJournalPos(m.undoSeq, m.storeMaxSeq) != m.cleanJournalPos
	if m.docID != 0 {
		if dirty {
			m.opentabs = m.opentabs.MarkDirtyByID(m.docID)
		} else {
			m.opentabs = m.opentabs.MarkCleanByID(m.docID)
		}
	} else if m.filePath != "" {
		if dirty {
			m.opentabs = m.opentabs.MarkDirty(m.filePath)
		} else {
			m.opentabs = m.opentabs.MarkClean(m.filePath)
		}
	}
	return m
}

func (m Model) finalize(cmds []tea.Cmd) (Model, tea.Cmd) {
	m = m.syncDirty()
	m = m.applyFocus()
	if m.totalWidth > 0 {
		m = m.recalcLayout()
	}
	return m, tea.Batch(cmds...)
}

func (m Model) finalizeLayoutChange(cmds []tea.Cmd) (Model, tea.Cmd) {
	m = m.applyFocus()
	if m.totalWidth > 0 {
		m = m.recalcLayout()
		var refreshCmd tea.Cmd
		m.editor, refreshCmd = m.editor.RefreshImagesAfterLayoutChange()
		if refreshCmd != nil {
			cmds = append(cmds, refreshCmd)
		}
	}
	return m, tea.Batch(cmds...)
}

// applyFocus projects the single focus authority (m.focus) onto every child's
// focus state. This is the ONLY place component focus is derived from the enum;
// it runs before every dispatch to children and on every Update exit.
func (m Model) applyFocus() Model {
	m.title = m.title.SetFocused(m.focus == paneTitle)
	m.filetree = m.filetree.SetFocused(m.focus == paneTree)
	m.opentabs = m.opentabs.SetFocused(m.focus == paneTabs)
	m.editor = m.editor.SetFocused(m.focus == paneCenter)
	m.chat = m.chat.SetFocused(m.focus == paneChat)
	m.search = m.search.SetFocused(m.focus == paneSearch)
	return m
}

func (m Model) syncCursorToFooter() Model {
	m.footer, _ = m.footer.Update(footer.UpdateCursorMsg{
		Line:      0,
		Col:       0,
		WordCount: 0,
	})
	return m
}

func (m Model) syncDictationAllowed() Model {
	m.footer = m.footer.SetDictationAllowed(m.focus == paneCenter || m.focus == paneChat)
	return m
}

func (m Model) handleUndo() (Model, tea.Cmd) {
	if m.store == nil || m.viewingHelp() {
		return m, nil
	}
	surface, edits, cursorsBefore, newPos, ok := m.store.UndoTarget(m.docID)
	if !ok {
		return m, nil
	}
	m.undoSeq = newPos
	var cmd tea.Cmd
	switch surface {
	case "main":
		m.editor, cmd = m.editor.ApplyInverse(edits)
		m.editor = m.editor.SetCursors(cursorsBefore)
	case "title":
		m.title = m.title.ApplyInverse(edits)
		m.title = m.title.SetCursors(cursorsBefore)
	case "chat":
		m.chat = m.chat.ApplyInverse(edits)
		m.chat = m.chat.SetCursors(cursorsBefore)
	}
	switch surface {
	case "main":
		m.focus = paneCenter
	case "title":
		m.focus = paneTitle
	case "chat":
		m.focus = paneChat
	}
	m = m.syncDictationAllowed()
	return m, cmd
}

func (m Model) handleRedo() (Model, tea.Cmd) {
	if m.store == nil || m.viewingHelp() {
		return m, nil
	}
	surface, edits, cursorsAfter, newPos, ok := m.store.RedoTarget(m.docID)
	if !ok {
		return m, nil
	}
	m.undoSeq = newPos
	var cmd tea.Cmd
	switch surface {
	case "main":
		m.editor, cmd = m.editor.Reapply(edits)
		m.editor = m.editor.SetCursors(cursorsAfter)
	case "title":
		m.title = m.title.Reapply(edits)
		m.title = m.title.SetCursors(cursorsAfter)
	case "chat":
		m.chat = m.chat.Reapply(edits)
		m.chat = m.chat.SetCursors(cursorsAfter)
	}
	switch surface {
	case "main":
		m.focus = paneCenter
	case "title":
		m.focus = paneTitle
	case "chat":
		m.focus = paneChat
	}
	m = m.syncDictationAllowed()
	return m, cmd
}

// scheduleFlush debounces VFS autosave. It increments the generation counter
// and launches a goroutine that sleeps for flushDelay then returns
// pendingFlushMsg. The handler drops stale generations (gen != m.flushGen)
// so only the last scheduled flush fires a snapshot (N5).
func (m Model) scheduleFlush(cmds *[]tea.Cmd) Model {
	m.flushGen++
	gen := m.flushGen
	*cmds = append(*cmds, func() tea.Msg {
		time.Sleep(flushDelay)
		return pendingFlushMsg{gen: gen}
	})
	return m
}

// snapshotCmd writes a VFS snapshot for docID at headSeq and reports the result
// via AutosaveSettledMsg. Disk is NOT written here; that only happens on
// explicit ⌘S (§1.4.2).
func snapshotCmd(store *docstate.Store, docID int64, content string, headSeq, gen uint64) tea.Cmd {
	return func() tea.Msg {
		_, err := store.CreateSnapshot(docID, content, "local", int64(headSeq))
		return AutosaveSettledMsg{gen: gen, err: err}
	}
}

// journalEdit appends an edit to the per-document journal and schedules VFS
// autosave. Call after DrainEdits returns non-empty edits.
func (m Model) journalEdit(surface string, edits []buffer.AppliedEdit, cursorsBefore, cursorsAfter []cursor.Cursor, cmds *[]tea.Cmd) Model {
	if m.store == nil || m.docID == 0 || len(edits) == 0 {
		return m
	}
	seq, err := m.store.AppendEdit(m.docID, surface, edits, cursorsBefore, cursorsAfter, surface)
	if err != nil {
		// §1.3: a failed journal write leaves undo history incomplete, and a
		// snapshot taken afterward would tag the new buffer against a stale head
		// seq — re-opening the B2/N5 corruption window. Surface the error and do
		// NOT schedule a snapshot for this edit.
		capturedErr := err
		*cmds = append(*cmds, func() tea.Msg {
			return footer.ShowErrorMsg{Text: "journal write failed: " + capturedErr.Error()}
		})
		return m
	}
	if seq > 0 {
		m.headSeq = seq // N5: track latest seq for snapshot co-tagging
		if seq > m.storeMaxSeq {
			// New event (AUTOINCREMENT guarantees seq > any prior max).
			// If we were mid-undo (truncation occurred) and the truncation
			// point was the clean state, the parent of this new event IS the
			// clean state, so update cleanJournalPos to seq-1 (the position
			// you'd land on after undoing this new event).
			if m.undoSeq >= 0 && effectiveJournalPos(m.undoSeq, m.storeMaxSeq) == m.cleanJournalPos {
				m.cleanJournalPos = seq - 1
			}
			m.storeMaxSeq = seq
		} else {
			// Coalescing updated an existing event in-place (seq ≤ old max).
			// If that event is at or before the save point, the save-point
			// event's content has changed — force dirty until the next save.
			m.storeMaxSeq = seq
			if seq <= m.cleanJournalPos {
				m.cleanJournalPos = -1
			}
		}
		m.undoSeq = -1 // AppendEdit always resets current_seq to NULL
	}
	if surface == "main" {
		m = m.scheduleFlush(cmds)
	}
	return m
}
