package workspace

import (
	"fmt"
	"path/filepath"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"

	dictengine "rune/pkg/dictation"
	"rune/pkg/docstate"
	"rune/pkg/editor/buffer"
	dictcomp "rune/pkg/ui/components/dictation"
	"rune/pkg/ui/components/filetree"
	"rune/pkg/ui/components/footer"
	"rune/pkg/ui/components/markdownedit"
	"rune/pkg/ui/components/opentabs"
	searchcomp "rune/pkg/ui/components/search"
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

		// Enforce the hard tab cap before doing anything else with this file.
		var limitCmd tea.Cmd
		var proceed bool
		m, limitCmd, proceed = m.enforceTabLimit(docID, msg.Path)
		cmds = append(cmds, limitCmd)
		if !proceed {
			break
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
		var dimg tea.Cmd
		m.editor, dimg = m.editor.DiscoverImages()
		cmds = append(cmds, dimg)
		m.editor = m.editor.SetReadOnly(false)
		m.filePath = msg.Path
		m.docID = docID
		m.baseline = msg.Baseline // §1.4.7: fingerprint for the external-change guard on ⌘S
		m.breadcrumb = m.breadcrumb.SetPath(msg.Path)
		m.opentabs = m.opentabs.OpenFile(docID, msg.Path)
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
			if m.store != nil && m.docID != 0 {
				_ = m.store.MarkSaved(m.docID) // fire-and-forget: §1.3
			}
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
		// Eviction background save ack: victim is clean, close it, open pending file.
		if m.isEvictSaveAck(msg.RequestID) {
			var openCmd tea.Cmd
			m, openCmd = m.evictSaveAck()
			cmds = append(cmds, openCmd)
			break
		}
		// A materialize from the multi-tab quit "Save all" batch.
		if m.pendingDataLoss.kind == actionQuit && m.pendingDataLoss.saveLeft > 0 {
			m.opentabs = m.opentabs.MarkCleanByID(msg.DocID)
			// Keep the saved doc's recorded inode in sync (atomic write changed it),
			// so a later session reopens it without orphaning its history.
			if m.store != nil && msg.DocID != 0 {
				_ = m.store.Bind(msg.DocID, msg.Path) // fire-and-forget: best-effort on quit
				_ = m.store.MarkSaved(msg.DocID)      // fire-and-forget: §1.3
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
		// Eviction background save failed — the pending file does not open;
		// the victim tab stays open, the user can act on it manually.
		if m.isEvictSaveAck(msg.RequestID) {
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
					// Upgrade the tab that was created with DocID==0 before the
					// store was ready. Mirrors ensureScratchDoc for the untitled case.
					m.opentabs = m.opentabs.AssignDocID(m.filePath, ref.ID)
				}
			} else {
				// Make the store-less startup untitled durable (§1.4.3).
				m = m.ensureScratchDoc()
			}
			// Surface prior-session unsaved untitled docs; GC empty scratch rows.
			m = m.restoreScratch()

			// Wire the history loader now that the store is ready.
			store := m.store
			m.search = m.search.WithHistoryLoader(func() ([]string, error) {
				return store.SearchHistory()
			})
		}

	case pendingFlushMsg:
		// Only the most recent flush request fires a snapshot (debounce). Earlier
		// goroutines return stale gen values and are dropped here.
		if msg.gen == m.flushGen && m.store != nil && m.docID > 0 {
			content := m.editor.Content()
			// Capture seq synchronously, co-atomic with content, so the snapshot
			// is tagged at the exact journal position the content reflects.
			// Never read CurrentSeq inside the goroutine — later AppendEdits on
			// the main loop would advance the head, tagging old content with a
			// newer seq and corrupting RecoverDocument (plan §C, CRITIC #3).
			seq, _ := m.store.CurrentSeq(m.docID)
			gen := msg.gen
			cmds = append(cmds, snapshotCmd(m.store, m.docID, content, uint64(seq), gen))
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
		return m.handleMouseClick(msg, cmds)

	case tea.MouseMotionMsg:
		return m.handleMouseMotion(msg, cmds)

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
			if m.pendingDataLoss.kind == actionEvict {
				// Evict: save the dirty background victim; FileSavedMsg closes it + opens pending.
				m, cmd = m.evictSave()
				cmds = append(cmds, cmd)
				break
			}
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
				var closeCmd tea.Cmd
				m, closeCmd = m.executeClose(m.docID, m.filePath)
				cmds = append(cmds, closeCmd)
			case actionEvict:
				// Discard: close the victim (history stays in VFS; recoverable on reopen).
				var discardCmd tea.Cmd
				m, discardCmd = m.evictDiscard(action)
				cmds = append(cmds, discardCmd)
			default: // actionQuit: discard all — journaled work survives in the VFS
				return m.teardownAndQuit()
			}

		case footer.DataLossCancel:
			m.pendingDataLoss = pendingDataLoss{}
			// Explicitly clear the guard: in production the footer already cleared
			// it before emitting this message; in tests that inject the response
			// directly the footer may not have, so clear it here unconditionally.
			m.footer = m.footer.SetGuard(footer.GuardDirty, nil)
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

	case searchcomp.SubmitMsg:
		// Enter / Shift+Enter in the search bar — navigate to a match and persist the query.
		if msg.Backward {
			m.editor = m.editor.FindPrev()
		} else {
			m.editor = m.editor.FindNext()
		}
		idx, total := m.editor.MatchCount()
		m.search = m.search.SetStatus(searchcomp.StatusFor(idx, total))
		if m.store != nil && msg.Query != "" {
			cmds = append(cmds, persistSearchQueryCmd(m.store, msg.Query))
		}

	case searchcomp.CloseMsg:
		// Escape from the search bar — clear highlights and return focus to editor.
		m.search = m.search.Close()
		m.editor = m.editor.ClearSearch()
		m.focus = paneCenter
		m = m.syncDictationAllowed()

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

		prevSearchQuery := m.search.Query()
		m.search, cmd = m.search.Update(msg)
		cmds = append(cmds, cmd)
		if q := m.search.Query(); q != prevSearchQuery && m.search.Visible() {
			m.editor = m.editor.SetSearchQuery(q, true)
			idx, total := m.editor.MatchCount()
			m.search = m.search.SetStatus(searchcomp.StatusFor(idx, total))
		}

		m.chat, cmd = m.chat.Update(msg)
		cmds = append(cmds, cmd)

		m.footer, cmd = m.footer.Update(msg)
		cmds = append(cmds, cmd)

		m = m.syncCursorToFooter()
	}

	return m.finalize(cmds)
}

// persistSearchQueryCmd writes a query to the search_history table asynchronously.
// Errors are silently swallowed — history persistence is best-effort.
func persistSearchQueryCmd(store *docstate.Store, query string) tea.Cmd {
	return func() tea.Msg {
		_ = store.AppendSearchQuery(query) // fire-and-forget: search history loss is tolerable
		return nil
	}
}
