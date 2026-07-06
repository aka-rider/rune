package workspace

import (
	"fmt"

	"charm.land/bubbles/v2/key"
	tea "charm.land/bubbletea/v2"

	"rune/pkg/editor/buffer"
	searchcomp "rune/pkg/ui/components/search"
	"rune/pkg/ui/pages/workspace/mergemode"
)

// handleKeyPress processes a tea.KeyPressMsg. It is called from Update() with
// the cmds slice already containing any dictation-drain commands so they are
// included in the returned tea.Batch.
func (m Model) handleKeyPress(msg tea.KeyPressMsg, cmds []tea.Cmd) (Model, tea.Cmd) {
	var cmd tea.Cmd

	// Priority 2: Save in-flight — consume all keys silently.
	if m.activeSave.InFlight {
		return m.finalize(cmds)
	}

	// Priority 2.1: a data-loss guard owns the keyboard until resolved (§1.4.4).
	// While the guard is showing, NO global key (save/close/new/undo/redo) may act
	// behind it — route the key only to the footer, which resolves on s/d/Esc and
	// ignores everything else. Without this, ⌘S behind an open dirty-close guard
	// issues a real save whose ack (pendingDataLoss still actionClose) then closes
	// and blanks the buffer — a destructive transition the user never confirmed.
	if m.footer.InGuard() {
		m.footer, cmd = m.footer.Update(msg)
		cmds = append(cmds, cmd)
		return m.finalize(cmds)
	}

	// Priority 2.5: Global undo/redo (skipped when search is focused — the
	// search component handles Undo internally for its own text field).
	switch {
	case key.Matches(msg, m.keys.Undo):
		if m.focus == paneSearch {
			break
		}
		var undoCmd tea.Cmd
		m, undoCmd = m.handleUndo()
		cmds = append(cmds, undoCmd)
		return m.finalize(cmds)
	case key.Matches(msg, m.keys.Redo):
		if m.focus == paneSearch {
			break
		}
		var redoCmd tea.Cmd
		m, redoCmd = m.handleRedo()
		cmds = append(cmds, redoCmd)
		return m.finalize(cmds)
	}

	// Priority 3: Global workspace keys.
	consumed := true
	switch {
	case key.Matches(msg, m.keys.SaveFile):
		if m.view.IsFile() && !m.activeSave.InFlight {
			m, cmd = m.startSave()
			cmds = append(cmds, cmd)
		}

	case key.Matches(msg, m.keys.TabSwitch):
		var ok bool
		m, cmds, ok = m.withFinalizedTitle(cmds)
		if !ok {
			return m.finalize(cmds)
		}
		digit := msg.BaseCode
		if digit == 0 {
			digit = msg.Code
		}
		idx := -1
		if digit >= '1' && digit <= '9' {
			idx = int(digit - '1')
		} else if digit == '0' {
			idx = 9
		}
		if idx >= 0 && idx < m.opentabs.Len() {
			path := m.opentabs.PathAt(idx)
			docID := m.opentabs.DocIDAt(idx)
			m.opentabs = m.opentabs.SelectIndex(idx)
			m, cmd = m.requestOpenPath(docID, path)
			cmds = append(cmds, cmd)
		}

	case key.Matches(msg, m.keys.PinTab):
		var ok bool
		m, cmds, ok = m.withFinalizedTitle(cmds)
		if !ok {
			return m.finalize(cmds)
		}
		m.opentabs = m.opentabs.PinIndex(m.opentabs.Cursor())

	case key.Matches(msg, m.keys.FocusExplorer):
		var ok bool
		m, cmds, ok = m.withFinalizedTitle(cmds)
		if !ok {
			return m.finalize(cmds)
		}
		m = m.setFocus(paneTree)
		m.leftVisible = true
		m = m.syncDictationAllowed()

	case key.Matches(msg, m.keys.FocusEditor):
		var ok bool
		m, cmds, ok = m.withFinalizedTitle(cmds)
		if !ok {
			return m.finalize(cmds)
		}
		m = m.setFocus(paneCenter)
		m = m.syncDictationAllowed()

	case key.Matches(msg, m.keys.FocusChat):
		var ok bool
		m, cmds, ok = m.withFinalizedTitle(cmds)
		if !ok {
			return m.finalize(cmds)
		}
		if m.rightVisible && m.focus == paneChat {
			m.rightVisible = false
			m = m.setFocus(paneCenter)
		} else {
			m.rightVisible = true
			m = m.setFocus(paneChat)
		}
		m = m.syncDictationAllowed()

	case key.Matches(msg, m.keys.CreateNewFile):
		// Modal merge (§4): ⌘N → CreateUntitled SetContent("")s over the hidden
		// marker buffer, backgrounding the mid-merge doc (its markers then sit in
		// the store and a later quit-save could reach disk). Refuse early — before
		// maybeFinalizeTitle and the setFocus(paneTitle) below — Esc-abort first.
		if m.HasUnresolvedConflicts() {
			m, cmd = m.refuseMergeTransition("creating a new file")
			cmds = append(cmds, cmd)
			return m.finalize(cmds)
		}
		var ok bool
		m, cmds, ok = m.withFinalizedTitle(cmds)
		if !ok {
			return m.finalize(cmds)
		}
		// The outgoing untitled (if any) stays as its own durable VFS doc/tab —
		// nothing is written to disk and nothing is lost (Fix 2 §5).
		if !m.view.IsUntitled() || m.editor.Content() != "" {
			m, cmd = m.CreateUntitled()
			cmds = append(cmds, cmd)
		}
		m.title = m.title.FocusAndSelectAll()
		m = m.setFocus(paneTitle)
		m = m.syncDictationAllowed()

	case key.Matches(msg, m.keys.CloseFile):
		var ok bool
		m, cmds, ok = m.withFinalizedTitle(cmds)
		if !ok {
			return m.finalize(cmds)
		}
		m, cmd = m.requestCloseCurrent()
		cmds = append(cmds, cmd)

	case key.Matches(msg, m.keys.Help):
		var ok bool
		m, cmds, ok = m.withFinalizedTitle(cmds)
		if !ok {
			return m.finalize(cmds)
		}
		m, cmd = m.toggleHelp()
		cmds = append(cmds, cmd)

	case key.Matches(msg, m.keys.ZenMode):
		var ok bool
		m, cmds, ok = m.withFinalizedTitle(cmds)
		if !ok {
			return m.finalize(cmds)
		}
		m.leftVisible = !m.leftVisible
		if !m.leftVisible && m.focus.isLeft() {
			m = m.setFocus(paneCenter)
		}
		m = m.syncDictationAllowed()

	case key.Matches(msg, m.keys.FindOpen):
		// Cmd+Shift+F / ^F — open (or toggle) the search bar.
		if m.search.Visible() {
			// Second press closes the bar and returns focus to the editor.
			m.search = m.search.Close()
			m.editor = m.editor.ClearSearch()
			m = m.setFocus(paneCenter)
		} else {
			m.search = m.search.Open()
			m = m.setFocus(paneSearch)
			m = m.recalcLayout()
		}
		m = m.syncDictationAllowed()

	case key.Matches(msg, m.keys.FindNext):
		// ⌘G — navigate to the next match (works with bar closed).
		m.editor = m.editor.FindNext()
		idx, total := m.editor.MatchCount()
		m.search = m.search.SetStatus(searchcomp.StatusFor(idx, total))

	case key.Matches(msg, m.keys.FindPrev):
		// ⇧⌘G — navigate to the previous match (works with bar closed).
		m.editor = m.editor.FindPrev()
		idx, total := m.editor.MatchCount()
		m.search = m.search.SetStatus(searchcomp.StatusFor(idx, total))

	default:
		consumed = false
	}

	if consumed {
		// Chord-sequence keys (ConfirmExitC/D) route to footer's internal
		// state machine even when already consumed as a global action. The
		// footer owns the chord state and renders the confirmation feedback.
		if key.Matches(msg, m.keys.ConfirmExitC) || key.Matches(msg, m.keys.ConfirmExitD) {
			m.footer, cmd = m.footer.Update(msg)
			cmds = append(cmds, cmd)
		}
		return m.finalize(cmds)
	}

	// Project focus before any key reaches a child (§3.3).
	m = m.applyFocus()

	// D11 — Up at editor top transfers focus to title.
	if m.focus == paneCenter && !m.viewingHelp() && msg.Code == tea.KeyUp && msg.Mod == 0 && m.editor.CursorAtTop() {
		m = m.setFocus(paneTitle)
		m.title = m.title.FocusAtEnd()
		return m.finalize(cmds)
	}

	// Singular key routing — exactly one child receives the KeyPressMsg (§3.3).
	switch m.focus {
	case paneTitle:
		prevCursors := m.title.Cursors()
		m.title, cmd = m.title.Update(msg)
		cmds = append(cmds, cmd)
		var titleEdits []buffer.AppliedEdit
		m.title, titleEdits = m.title.DrainEdits()
		m = m.journalEdit("title", titleEdits, prevCursors, m.title.Cursors(), &cmds)
	case paneCenter:
		if !m.dict.Enabled() {
			prevCursors := m.editor.Cursors()
			// Merge resolver pre-intercept (§4): while active, [O]/[T]/[n]/[p]/
			// scroll keys resolve against the MAIN editor (the hidden marker
			// buffer) — mirroring the dictation drain precedent above. Esc
			// aborts the modal merge (reverts to pre-merge ours); every other
			// key is consumed by mergemode so no free-text edit reaches the
			// (hidden) buffer mid-merge.
			var consumed bool
			if mergemode.IsActive(m.merge) {
				if key.Matches(msg, m.keys.Cancel) {
					var abortErr error
					m.merge, m.editor, cmd, abortErr = mergemode.Abort(m.merge, m.editor)
					if abortErr != nil {
						// D10: this used to `return m.finalize(cmds)` here,
						// early-exiting handleKeyPress and skipping the
						// footer-routing / syncCursorToFooter / syncMergeHint
						// tail below that every OTHER key path (including the
						// sibling mergemode.HandleKey error branch just below)
						// still reaches — the footer's chord/help state
						// machine never saw this keypress at all. Surface the
						// error and fall through like every other path
						// instead of returning early.
						cmds = append(cmds, errorCmd(fmt.Errorf("abort merge: %w", abortErr)))
						consumed = true
					} else {
						// Part IV: reverting to pre-merge content is a
						// wholesale buffer install, invalidating every
						// outstanding view ticket — conservative/safety-
						// additive even though the plan's four listed
						// transition classes don't name Abort specifically
						// (worst case a still-safe async result is
						// needlessly refused, never a data-loss regression).
						m = m.bumpEpoch()
						// F3: undo the CAS-baseline adoption entering merge
						// made (resolveAdoptAt) and re-probe fresh, so a
						// dismissed merge re-raises the Diverged guard
						// immediately instead of leaving a stale "resolved"
						// baseline a later ⌘S could silently write over
						// theirs with.
						m = m.abandonMergeResolve(&cmds)
						consumed = true
					}
				} else {
					var handleErr error
					m.merge, m.editor, cmd, consumed, handleErr = mergemode.HandleKey(m.merge, m.editor, msg)
					if handleErr != nil {
						cmds = append(cmds, errorCmd(fmt.Errorf("merge resolve: %w", handleErr)))
					}
				}
				cmds = append(cmds, cmd)
			}
			if !consumed {
				m.editor, cmd = m.editor.Update(msg)
				cmds = append(cmds, cmd)
			}
			var editorEdits []buffer.AppliedEdit
			m.editor, editorEdits = m.editor.DrainEdits()
			m = m.journalEdit("main", editorEdits, prevCursors, m.editor.Cursors(), &cmds)
		}
	case paneChat:
		prevCursors := m.chat.Cursors()
		m.chat, cmd = m.chat.Update(msg)
		cmds = append(cmds, cmd)
		var chatEdits []buffer.AppliedEdit
		m.chat, chatEdits = m.chat.DrainEdits()
		m = m.journalEdit("chat", chatEdits, prevCursors, m.chat.Cursors(), &cmds)
	case paneSearch:
		prevQuery := m.search.Query()
		m.search, cmd = m.search.Update(msg)
		cmds = append(cmds, cmd)
		if q := m.search.Query(); q != prevQuery {
			// Live update: apply new query to editor and refresh status.
			m.editor = m.editor.SetSearchQuery(q, true)
			idx, total := m.editor.MatchCount()
			m.search = m.search.SetStatus(searchcomp.StatusFor(idx, total))
		}
	case paneTree:
		m.filetree, cmd = m.filetree.Update(msg)
		cmds = append(cmds, cmd)
	case paneTabs:
		m.opentabs, cmd = m.opentabs.Update(msg)
		cmds = append(cmds, cmd)
	}
	// Footer always receives keys for chord/help state machine (§3.2).
	m.footer, cmd = m.footer.Update(msg)
	cmds = append(cmds, cmd)

	m = m.syncCursorToFooter()
	m = m.syncMergeHint()
	return m.finalize(cmds)
}
