package workspace

import (
	"fmt"
	"time"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/docstate"
	"rune/pkg/editor/buffer"
	"rune/pkg/editor/cursor"
	"rune/pkg/ui/components/footer"
	"rune/pkg/ui/components/opentabs"
)

func (m Model) handleMouseClick(msg tea.MouseClickMsg, cmds []tea.Cmd) (Model, tea.Cmd) {
	m.drag = dragNone

	if d, ok := m.dividerAtPoint(msg.X, msg.Y); ok {
		m.drag = d
		if d == dragLeft && !m.leftVisible {
			m.leftVisible = true
			m.leftPaneW = minLeftPaneW
		} else if d == dragRight && !m.rightVisible {
			m.rightVisible = true
			m.rightPaneW = minRightPaneW
		}
		return m.finalizeLayoutChange(cmds)
	}
	if newFocus, ok := m.paneAtPoint(msg.X, msg.Y); ok {
		if newFocus == paneTitle {
			m.focus = paneTitle
			m.title = m.title.FocusAtEnd()
		} else {
			if m.focus == paneTitle {
				var finalizeCmd tea.Cmd
				var finalizeOk bool
				m, finalizeCmd, finalizeOk = m.maybeFinalizeTitle()
				cmds = append(cmds, finalizeCmd)
				if !finalizeOk {
					return m.finalize(cmds)
				}
			}
			m.focus = newFocus
			if newFocus == paneCenter {
				// Forward the click to the editor so it positions the caret,
				// follows a link, or sets the drag anchor. applyFocus is the only
				// sanctioned focus projection (pkg/ui/CLAUDE.md §2); the editor
				// gates on Focused() before acting on the click.
				m = m.applyFocus()
				var ecmd tea.Cmd
				m.editor, ecmd = m.editor.Update(msg)
				cmds = append(cmds, ecmd)
				// Refresh the link-under-caret hint now that the caret moved —
				// finalize() does not, so otherwise it'd be stale until a keypress.
				m = m.syncCursorToFooter()
			}
		}
		m = m.syncDictationAllowed()
	}
	return m.finalize(cmds)
}

func (m Model) handleMouseMotion(msg tea.MouseMotionMsg, cmds []tea.Cmd) (Model, tea.Cmd) {
	if m.drag == dragNone {
		// Not dragging a pane divider — forward a left-button drag to the editor
		// for text selection (it extends from the anchor the click set).
		if msg.Button == tea.MouseLeft && m.focus == paneCenter {
			var ecmd tea.Cmd
			m.editor, ecmd = m.editor.Update(msg)
			cmds = append(cmds, ecmd)
		}
		return m.finalize(cmds)
	}
	if msg.Button != tea.MouseLeft {
		m.drag = dragNone
		return m.finalize(cmds)
	}
	switch m.drag {
	case dragLeft:
		newW := msg.X
		if newW < minLeftPaneW {
			m.leftVisible = false
			m.leftPaneW = defaultLeftPaneW
			m.drag = dragNone
			if m.focus.isLeft() {
				m.focus = paneCenter
				m = m.syncDictationAllowed()
			}
		} else {
			rightW := 0
			if m.rightVisible {
				rightW = m.rightPaneW
			}
			if max := m.totalWidth - rightW - minCenterW; newW > max {
				newW = max
			}
			m.leftPaneW = newW
			m.leftVisible = true
		}
	case dragRight:
		newW := m.totalWidth - msg.X
		if newW < minRightPaneW {
			m.rightVisible = false
			m.rightPaneW = defaultRightPaneW
			m.drag = dragNone
			if m.focus == paneChat {
				m.focus = paneCenter
				m = m.syncDictationAllowed()
			}
		} else {
			leftW := 0
			if m.leftVisible {
				leftW = m.leftPaneW
			}
			if max := m.totalWidth - leftW - minCenterW; newW > max {
				newW = max
			}
			m.rightPaneW = newW
			m.rightVisible = true
		}
	}
	return m.finalizeLayoutChange(cmds)
}

// savedSeqFor returns the journal position docID currently reflects, read
// SYNCHRONOUSLY (co-atomic with the content the caller captured) so a save stamps
// saved_seq at the position the bytes it writes correspond to — never the live head
// a later edit advances to while the async write is in flight (§1.4.2/§1.4.8).
// 0 when there is no store/doc.
func (m Model) savedSeqFor(docID int64) int64 {
	if m.store == nil || docID == 0 {
		return 0
	}
	seq, err := m.store.CurrentSeq(docID)
	if err != nil {
		return 0 // fire-and-forget: read error → seq 0; MarkSavedAt stays conservative
	}
	return seq
}

func (m Model) startSave() (Model, tea.Cmd) {
	// Inert while a load is pending: the editor buffer may not yet match the
	// incoming identity (close→neighbour transition), so a save now could write
	// the wrong bytes. The gate clears on the load result (§1.4).
	// Not a file (untitled / help / transitional 0/""), a save in flight, or a load
	// pending → inert. !IsFile() also makes the read-only help structurally
	// non-saveable. The gate clears on the load result (§1.4).
	if !m.view.IsFile() || m.activeSave.InFlight || m.pendingLoad.active {
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
	return m, materializeCmd(m.fsys(), m.view.DocID(), m.view.Path(), content, m.savedSeqFor(m.view.DocID()), requestID, false, m.view.Baseline())
}

func (m Model) syncDirty() Model {
	if m.viewingHelp() || m.store == nil || m.view.DocID() == 0 {
		return m // no DB record — last-known mark stands
	}
	dirty, err := m.store.IsDirty(m.view.DocID())
	if err != nil {
		// fire-and-forget: dirty is a rung-3 display indicator; the journal is the durable truth
		return m
	}
	if dirty {
		m.opentabs = m.opentabs.MarkDirtyByID(m.view.DocID())
	} else {
		m.opentabs = m.opentabs.MarkCleanByID(m.view.DocID())
	}
	return m
}

func (m Model) finalize(cmds []tea.Cmd) (Model, tea.Cmd) {
	m = m.syncDirty()
	// TAB-SET: mirror the active tab to the live identity. During a close→neighbour
	// transition the live identity is the save-safe 0/"" (executeClose left it
	// there), so derive the active tab + link base dir from the pending target
	// instead — the active handle intentionally LEADS the identity by one async
	// hop here (INV-ACTIVE-SYNC holds only after settle; do NOT "fix" it by
	// tracking live identity — that reintroduces the stranding). Every other load
	// path keeps a non-empty live identity, so it is unaffected.
	active := m.view.Handle()
	if m.pendingLoad.active && m.view.IsUntitled() && m.view.DocID() == 0 {
		active = opentabs.TabHandle{DocID: m.pendingLoad.docID, Path: m.pendingLoad.path}
	}
	m.opentabs = m.opentabs.SetActive(active)
	// Project the editor's link/embed base from the single source (m.view) at this
	// one authority point — like applyFocus projects m.focus. The GOLDEN path
	// verbatim (the editor derives Dir() itself), so it tracks every settled
	// transition (load/untitled/help/bind/rename) and never drifts.
	m.editor = m.editor.SetDocPath(m.view.Path())
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
	// Surface the link under the caret as a footer hint, but only while the
	// editor is focused (the caret is meaningless to the user otherwise).
	linkTarget := ""
	if m.focus == paneCenter {
		linkTarget, _ = m.editor.LinkAtCursor()
	}
	m.footer, _ = m.footer.Update(footer.UpdateCursorMsg{LinkTarget: linkTarget})
	return m
}

func (m Model) syncDictationAllowed() Model {
	m.footer = m.footer.SetDictationAllowed(m.focus == paneCenter || m.focus == paneChat)
	return m
}

// errorCmd surfaces err on the footer status line. Per §1.3 a buffer-edit failure
// is a Tolerable halt that keeps the buffer — never a silent drop.
func errorCmd(err error) tea.Cmd {
	text := err.Error()
	return func() tea.Msg { return footer.ShowErrorMsg{Text: text} }
}

// handleUndo applies one undo step with journal⇄buffer coherence (§1.4.8): it
// PEEKS the target (without moving the journal), applies the inverse to the
// buffer, and commits the position move ONLY if the buffer edit succeeds. A
// failed reapply (stale/out-of-bounds positions — the buffer and journal already
// diverged) surfaces a §1.3 error and leaves the position unmoved, so the journal
// never ends up ahead of the buffer.
func (m Model) handleUndo() (Model, tea.Cmd) {
	if m.store == nil || m.viewingHelp() {
		return m, nil
	}
	docID := m.view.DocID()
	surface, edits, cursorsBefore, newPos, ok := m.store.UndoPeek(docID)
	if !ok {
		return m, nil
	}
	var cmd tea.Cmd
	var err error
	switch surface {
	case "main":
		m.editor, cmd, err = m.editor.ApplyInverse(edits)
		if err == nil {
			m.editor = m.editor.SetCursors(cursorsBefore)
		}
	case "title":
		m.title, err = m.title.ApplyInverse(edits)
		if err == nil {
			m.title = m.title.SetCursors(cursorsBefore)
		}
	case "chat":
		m.chat, err = m.chat.ApplyInverse(edits)
		if err == nil {
			m.chat = m.chat.SetCursors(cursorsBefore)
		}
	}
	if err != nil {
		return m, errorCmd(fmt.Errorf("undo %s: %w", surface, err))
	}
	if cerr := m.store.MoveUndoPos(docID, newPos); cerr != nil {
		return m, errorCmd(fmt.Errorf("undo %s: commit position: %w", surface, cerr))
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

// handleRedo mirrors handleUndo: peek the redo target, reapply to the buffer, and
// advance the journal position only on a clean apply (§1.4.8). A failed reapply
// surfaces a §1.3 error and leaves the position unmoved.
func (m Model) handleRedo() (Model, tea.Cmd) {
	if m.store == nil || m.viewingHelp() {
		return m, nil
	}
	docID := m.view.DocID()
	surface, edits, cursorsAfter, newPos, ok := m.store.RedoPeek(docID)
	if !ok {
		return m, nil
	}
	var cmd tea.Cmd
	var err error
	switch surface {
	case "main":
		m.editor, cmd, err = m.editor.Reapply(edits)
		if err == nil {
			m.editor = m.editor.SetCursors(cursorsAfter)
		}
	case "title":
		m.title, err = m.title.Reapply(edits)
		if err == nil {
			m.title = m.title.SetCursors(cursorsAfter)
		}
	case "chat":
		m.chat, err = m.chat.Reapply(edits)
		if err == nil {
			m.chat = m.chat.SetCursors(cursorsAfter)
		}
	}
	if err != nil {
		return m, errorCmd(fmt.Errorf("redo %s: %w", surface, err))
	}
	if cerr := m.store.MoveUndoPos(docID, newPos); cerr != nil {
		return m, errorCmd(fmt.Errorf("redo %s: commit position: %w", surface, cerr))
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

// snapshotCmd writes a VFS snapshot for docID at the given journal seq (the
// document's current position, captured synchronously by the caller — it is NOT
// always the head, e.g. after an undo) and reports the result via
// AutosaveSettledMsg. Disk is NOT written here; that only happens on explicit ⌘S
// (§1.4.2).
func snapshotCmd(store *docstate.Store, docID int64, content string, seq, gen uint64) tea.Cmd {
	return func() tea.Msg {
		_, err := store.CreateSnapshot(docID, content, "local", int64(seq))
		return AutosaveSettledMsg{gen: gen, err: err}
	}
}

// journalEdit appends an edit to the per-document journal and schedules VFS
// autosave. Call after DrainEdits returns non-empty edits.
func (m Model) journalEdit(surface string, edits []buffer.AppliedEdit, cursorsBefore, cursorsAfter []cursor.Cursor, cmds *[]tea.Cmd) Model {
	if m.store == nil || m.view.DocID() == 0 || len(edits) == 0 {
		return m
	}
	_, err := m.store.AppendEdit(m.view.DocID(), surface, edits, cursorsBefore, cursorsAfter, surface)
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
	if surface == "main" {
		m = m.scheduleFlush(cmds)
	}
	return m
}
