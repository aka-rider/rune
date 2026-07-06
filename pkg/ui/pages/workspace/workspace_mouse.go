package workspace

import (
	tea "charm.land/bubbletea/v2"
)

// handleMouseClick routes a mouse click to a divider drag, a pane focus
// change, or the focused child's own click handling. Split out of
// workspace_edit.go to keep that file under the 500-LoC limit (§1.6/§11).
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
			m = m.setFocus(paneTitle)
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
			// setFocus is the single chokepoint: focus enum + projection onto children
			// happen atomically, so each child's handleMouseClick sees Focused()==true
			// for the intended target when we forward the click below.
			m = m.setFocus(newFocus)
			var ftcmd, tabcmd, ecmd, chatcmd tea.Cmd
			m.filetree, ftcmd = m.filetree.Update(msg)
			m.opentabs, tabcmd = m.opentabs.Update(msg)
			m.editor, ecmd = m.editor.Update(msg)
			m.chat, chatcmd = m.chat.Update(msg)
			cmds = append(cmds, ftcmd, tabcmd, ecmd, chatcmd)
			// Refresh the link-under-caret hint; syncCursorToFooter gates on
			// m.focus == paneCenter internally, so this is safe to call always.
			m = m.syncCursorToFooter()
		}
		m = m.syncDictationAllowed()
		m = m.syncMergeHint()
	}
	return m.finalize(cmds)
}

// handleMouseMotion drives a divider drag or forwards a left-button drag to
// the editor for text selection.
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
				m = m.setFocus(paneCenter)
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
				m = m.setFocus(paneCenter)
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
