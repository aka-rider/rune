package workspace

import (
	"fmt"
	"path/filepath"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"

	dictengine "rune/pkg/dictation"
	"rune/pkg/editor/buffer"
	dictcomp "rune/pkg/ui/components/dictation"
	"rune/pkg/ui/components/filetree"
	"rune/pkg/ui/components/footer"
	"rune/pkg/ui/components/markdownedit"
	"rune/pkg/ui/components/opentabs"
	"rune/pkg/ui/components/title"
)

func (m Model) Update(msg tea.Msg) (Model, tea.Cmd) {
	var cmds []tea.Cmd
	var cmd tea.Cmd

	// Always forward all messages to the dictation component (engine management).
	m.dict, cmd = m.dict.Update(msg)
	cmds = append(cmds, cmd)
	// Drain any pending edit from dictation and route to the focused target (D16).
	var s, e int
	var t string
	var hasPending bool
	m.dict, s, e, t, hasPending = m.dict.TakePendingEdit()
	if hasPending {
		switch m.focus {
		case paneCenter:
			prevCursors := m.editor.Cursors()
			m.editor, cmd = m.editor.ReplaceRange(s, e, t)
			cmds = append(cmds, cmd)
			var dictEdits []buffer.AppliedEdit
			m.editor, dictEdits = m.editor.DrainEdits()
			m = m.journalEdit("main", dictEdits, prevCursors, m.editor.Cursors(), &cmds)
		case paneChat:
			m.chat = m.chat.ApplyToPrompt(s, e, t)
		}
	}

	switch msg := msg.(type) {
	case tea.WindowSizeMsg:
		m.totalWidth, m.totalHeight = msg.Width, msg.Height
		m = m.recalcLayout()

	case tea.KeyPressMsg:
		return m.handleKeyPress(msg, cmds)

	case title.FocusReturnMsg:
		m.focus = paneCenter
		m = m.syncDictationAllowed()

	case title.RenameRequestMsg:
		if m.viewingHelp() {
			break
		}
		if m.filePath == "" {
			// First materialize of an untitled doc (bind-new). Do NOT set
			// m.filePath yet: materializeCmd refuses to clobber an existing file,
			// and binding optimistically would let a later ⌘S overwrite it
			// (Catastrophic, rung 1). The buffer stays untitled until the write
			// succeeds; FileSavedMsg{BindNew} performs the bind then.
			newPath := filepath.Join(m.currentDir(), msg.Name+".md")
			requestID := fmt.Sprintf("bind-%v", time.Now().UnixNano())
			m.activeSave = SaveIdentity{RequestID: requestID, SavedContent: []byte(m.editor.Content()), InFlight: true}
			cmds = append(cmds, materializeCmd(m.docID, newPath, m.editor.Content(), requestID, true, diskBaseline{}))
		} else {
			dir := filepath.Dir(m.filePath)
			newPath := filepath.Join(dir, msg.Name+".md")
			cmds = append(cmds, fileRenameCmd(m.filePath, newPath))
		}

	case filetree.FileSelectedMsg:
		m, cmd = m.requestOpenPath(0, msg.Path)
		cmds = append(cmds, cmd)

	case filetree.DirSelectedMsg:
		cmds = append(cmds, loadDirCmd(msg.Path))

	case filetree.DirLoadedMsg:
		m.editor = m.editor.SetDir(msg.Root)
		m.breadcrumb = m.breadcrumb.SetDir(msg.Root)
		m, cmd = m.startWatch(msg.Root)
		cmds = append(cmds, cmd)

	case dirChangedMsg:
		dir := m.watchedDir
		cmds = append(cmds, reloadDirCmd(dir))

	case opentabs.TabSelectedMsg:
		m, cmd = m.requestOpenPath(msg.DocID, msg.Path)
		cmds = append(cmds, cmd)

	case markdownedit.LinkClickedMsg:
		if msg.Path != "" {
			m, cmd = m.requestOpenPath(0, msg.Path)
			cmds = append(cmds, cmd)
		}

	case FileLoadedMsg:
		// Resolve VFS identity for this file (inode-keyed, rename-aware).
		var docID int64
		if msg.Path != "" && m.store != nil {
			if ref, err := m.store.OpenPath(msg.Path); err == nil {
				docID = ref.ID
				if ref.RenamedFrom != "" {
					m.opentabs = m.opentabs.RenameFile(ref.RenamedFrom, msg.Path)
				}
			}
		}

		// Prefer VFS reconstruction when the document has VFS history (§1.4.3).
		// HasHistory distinguishes "no VFS record" from "VFS record with empty content"
		// (e.g. user deleted all text, autosaved, then crashed — RecoverDocument
		// correctly returns "" which IS the intended content).
		content := string(msg.Content)
		if docID > 0 && m.store != nil {
			if hasHistory, err := m.store.HasHistory(docID); err == nil && hasHistory {
				if vfsContent, err := m.store.RecoverDocument(docID); err == nil && vfsContent != content {
					content = vfsContent
				}
			}
		}

		// Discard the empty untitled placeholder when transitioning to a real file.
		if m.filePath == "" && m.editor.Content() == "" {
			if m.docID != 0 {
				m.opentabs = m.opentabs.CloseByID(m.docID)
			} else {
				m.opentabs = m.opentabs.CloseFile("")
			}
		}
		m.editor = m.editor.SetContent(content)
		m.editor = m.editor.SetReadOnly(false)
		m.filePath = msg.Path
		m.docID = docID
		m.headSeq = 0
		m.baseline = msg.Baseline // §1.4.7: fingerprint for the external-change guard on ⌘S
		m.breadcrumb = m.breadcrumb.SetPath(msg.Path)
		m.opentabs = m.opentabs.OpenFile(docID, msg.Path)
		m.opentabs = m.opentabs.MarkCleanByID(docID)
		m.cleanRev = m.editor.Revision()
		m.chat = m.chat.SetFileContext(msg.Path, string(msg.Content))
		if msg.Path != "" {
			base := strings.TrimSuffix(filepath.Base(msg.Path), ".md")
			m.title = m.title.SetText(base)
		}

	case FileRenamedMsg:
		m.opentabs = m.opentabs.RenameFile(msg.OldPath, msg.NewPath)
		m.filePath = msg.NewPath
		m.baseline = baselineOf(msg.NewPath)
		// Rebind the VFS doc to the new path. os.Rename preserved the inode, so
		// OpenPath finds the same doc and just updates its path — preserving the
		// undo history. We initiated this rename, so the RenamedFrom warning is
		// expected and ignored.
		if m.store != nil {
			if ref, err := m.store.OpenPath(msg.NewPath); err == nil {
				m.docID = ref.ID
			}
		}
		m.breadcrumb = m.breadcrumb.SetPath(msg.NewPath)
		m.title = m.title.SetText(strings.TrimSuffix(filepath.Base(msg.NewPath), ".md"))

	case FileRenameErrorMsg:
		m.footer, cmd = m.footer.Update(footer.ShowErrorMsg{Text: msg.Err.Error()})
		cmds = append(cmds, cmd)

	case FileSavedMsg:
		// Interactive ⌘S, close-save, or bind-new (tracked by activeSave).
		if m.activeSave.InFlight && m.activeSave.RequestID == msg.RequestID {
			m.activeSave.InFlight = false
			m.baseline = msg.Baseline
			if msg.BindNew {
				m = m.bindMaterialized(msg.Path)
			} else if m.store != nil && m.docID != 0 {
				// Overwrite save: the atomic write gave the file a new inode.
				// Re-bind so the recorded identity stays in sync and the next
				// OpenPath resolves to THIS doc instead of orphaning its history.
				if err := m.store.Bind(m.docID, msg.Path); err != nil {
					m.err = fmt.Errorf("refresh binding for %q: %w", msg.Path, err)
				}
			}
			m.cleanRev = m.editor.Revision()
			if m.docID != 0 {
				m.opentabs = m.opentabs.MarkCleanByID(m.docID)
			} else {
				m.opentabs = m.opentabs.MarkClean(m.filePath)
			}
			if m.pendingDataLoss.kind == actionClose {
				m.pendingDataLoss = pendingDataLoss{}
				var closeCmd tea.Cmd
				m, closeCmd = m.executeClose(m.docID, m.filePath)
				cmds = append(cmds, closeCmd)
			}
			break
		}
		// A materialize from the multi-tab quit "Save all" batch.
		if m.pendingDataLoss.kind == actionQuit && m.pendingDataLoss.saveLeft > 0 {
			m.opentabs = m.opentabs.MarkCleanByID(msg.DocID)
			// Keep the saved doc's recorded inode in sync (atomic write changed it),
			// so a later session reopens it without orphaning its history.
			if m.store != nil && msg.DocID != 0 {
				_ = m.store.Bind(msg.DocID, msg.Path) // fire-and-forget: best-effort on quit
			}
			m.pendingDataLoss.saveLeft--
			if m.pendingDataLoss.saveLeft == 0 {
				return m.teardownAndQuit()
			}
		}

	case FileSaveErrorMsg:
		// Interactive / bind-new save failure: keep the buffer, surface the
		// conflict, and abort any pending close/quit so nothing is discarded.
		if m.activeSave.InFlight && m.activeSave.RequestID == msg.RequestID {
			m.activeSave.InFlight = false
			m.pendingDataLoss = pendingDataLoss{}
			m.footer, cmd = m.footer.Update(footer.ShowErrorMsg{Text: msg.Err.Error()})
			cmds = append(cmds, cmd)
			break
		}
		// A save in the multi-tab quit batch failed → abort the whole quit on the
		// first failure; every buffer is kept (durable in the VFS) and the
		// conflict is surfaced. Other in-flight saves still complete (their writes
		// succeeded); their acks are ignored now that the action is cleared.
		if m.pendingDataLoss.kind == actionQuit {
			m.pendingDataLoss = pendingDataLoss{}
			m.footer, cmd = m.footer.Update(footer.ShowErrorMsg{Text: msg.Err.Error()})
			cmds = append(cmds, cmd)
		}

	case FileLoadErrorMsg:
		// Ignore (e.g., broken link click to missing file)

	case fileWatchReadError:
		m.err = fmt.Errorf("external change to %s: %w", msg.path, msg.err)

	case StoreReadyMsg:
		m.store = msg.Store
		if msg.Warning != "" {
			m.footer, cmd = m.footer.Update(footer.ShowErrorMsg{Text: msg.Warning})
			cmds = append(cmds, cmd)
		}
		if m.store != nil {
			// Reserve the stable chat sentinel doc (N7).
			if chatID, err := m.store.ReserveChatDoc(); err == nil {
				m.chatDocID = chatID
			}
			if m.filePath != "" {
				// Bound file opened before the store was ready: resolve identity.
				if ref, err := m.store.OpenPath(m.filePath); err == nil {
					m.docID = ref.ID
				}
			} else {
				// Make the store-less startup untitled durable (§1.4.3).
				m = m.ensureScratchDoc()
			}
			// Surface prior-session unsaved untitled docs; GC empty scratch rows.
			m = m.restoreScratch()
		}

	case pendingFlushMsg:
		// Only the most recent flush request fires a snapshot (debounce). Earlier
		// goroutines return stale gen values and are dropped here.
		if msg.gen == m.flushGen && m.store != nil && m.docID > 0 {
			content := m.editor.Content()
			headSeq := uint64(m.headSeq)
			gen := msg.gen
			cmds = append(cmds, snapshotCmd(m.store, m.docID, content, headSeq, gen))
		}

	case AutosaveSettledMsg:
		// The VFS snapshot was written inside snapshotCmd; disk is untouched
		// (§1.4.2). Surface a snapshot failure — the journal remains durable, so
		// no data is lost, but the user should know history capture is degraded.
		if msg.err != nil {
			m.footer, cmd = m.footer.Update(footer.ShowErrorMsg{Text: "snapshot failed: " + msg.err.Error()})
			cmds = append(cmds, cmd)
		}

	case ErrMsg:
		m.err = msg.Err

	case tea.MouseClickMsg:
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
			}
			m = m.syncDictationAllowed()
		}

	case tea.MouseMotionMsg:
		if m.drag == dragNone {
			break
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

	case footer.ConfirmQuitMsg:
		if m.opentabs.HasDirty() {
			m.pendingDataLoss = pendingDataLoss{kind: actionQuit}
			m.footer = m.footer.SetGuard(footer.GuardDirty, dataLossGuardOptions)
			return m, nil
		}
		return m.teardownAndQuit()

	case footer.DataLossGuardResponseMsg:
		switch msg.Response {
		case footer.DataLossSave:
			if m.pendingDataLoss.kind == actionQuit {
				// Quit: materialize every dirty bound tab, then tear down.
				m, cmd = m.saveAllDirtyForQuit()
				cmds = append(cmds, cmd)
				break
			}
			// Close (or stray): save the current tab; FileSavedMsg closes it.
			if m.filePath == "" {
				// Untitled has no path to save to. Its work is durable in the VFS,
				// so keep the buffer and abort the close rather than lose anything.
				m.pendingDataLoss = pendingDataLoss{}
				m.footer, cmd = m.footer.Update(footer.ShowErrorMsg{Text: "Untitled — name it to save (its text is safe in history)"})
				cmds = append(cmds, cmd)
				break
			}
			m, cmd = m.startSave()
			cmds = append(cmds, cmd)
			// pendingDataLoss preserved — FileSavedMsg checks it to decide close.

		case footer.DataLossDiscard:
			action := m.pendingDataLoss
			m.pendingDataLoss = pendingDataLoss{}
			switch action.kind {
			case actionClose:
				// Discarding an untitled removes its VFS doc so it is not offered
				// for recovery later (Fix 7 §6); a bound doc keeps its history.
				if m.filePath == "" && m.docID != 0 && m.store != nil {
					if err := m.store.DeleteDoc(m.docID); err != nil {
						_ = err // fire-and-forget: discard cleanup; non-fatal
					}
				}
				m.cleanRev = m.editor.Revision()
				var closeCmd tea.Cmd
				m, closeCmd = m.executeClose(m.docID, m.filePath)
				cmds = append(cmds, closeCmd)
			default: // actionQuit: discard all — journaled work survives in the VFS
				return m.teardownAndQuit()
			}

		case footer.DataLossCancel:
			m.pendingDataLoss = pendingDataLoss{}
			// Guard already cleared by footer; nothing else to do.
		}

	case footer.DictationStartMsg:
		m.dict = m.dict.Enable(m.editor.CursorOffset())
		if m.focus == paneCenter {
			m.dict, cmd = m.dict.StartCmd()
			cmds = append(cmds, cmd)
		}

	case footer.DictationStopMsg:
		m.dict = m.dict.Disable()

	case dictcomp.DoneMsg:
		m.footer = m.footer.SetDictating(false)

	case dictengine.ErrorMsg:
		if msg.Fatal {
			m.footer = m.footer.SetDictating(false)
		}
	}

	// Forward non-key messages to all children (broadcast path).
	if _, isKey := msg.(tea.KeyPressMsg); !isKey {
		m = m.applyFocus()

		m.title, cmd = m.title.Update(msg)
		cmds = append(cmds, cmd)

		m.filetree, cmd = m.filetree.Update(msg)
		cmds = append(cmds, cmd)

		m.opentabs, cmd = m.opentabs.Update(msg)
		cmds = append(cmds, cmd)

		m.editor, cmd = m.editor.Update(msg)
		cmds = append(cmds, cmd)

		m.chat, cmd = m.chat.Update(msg)
		cmds = append(cmds, cmd)

		m.footer, cmd = m.footer.Update(msg)
		cmds = append(cmds, cmd)

		m = m.syncCursorToFooter()
	}

	return m.finalize(cmds)
}
