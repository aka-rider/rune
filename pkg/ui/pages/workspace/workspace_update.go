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
		// E1 (data safety): mid-merge, routeDictationEdit refuses to drain into
		// the hidden marker buffer — see its doc comment (workspace_merge_fresh.go).
		m = m.routeDictationEdit(s, e, t, &cmds)
	}

	switch msg := msg.(type) {
	case tea.WindowSizeMsg:
		m.totalWidth, m.totalHeight = msg.Width, msg.Height
		m = m.recalcLayout()

	case tea.KeyPressMsg:
		return m.handleKeyPress(msg, cmds)

	case title.FocusReturnMsg:
		m = m.setFocus(paneCenter)
		m = m.syncDictationAllowed()

	case title.RenameRequestMsg:
		if m.viewingHelp() {
			break
		}
		// Modal merge (§4): refuse renaming/bind-new-materializing the active
		// doc while unresolved — Esc-abort is the escape hatch.
		if m.HasUnresolvedConflicts() {
			m.footer, cmd = m.footer.Update(footer.ShowErrorMsg{Text: "Resolve or Esc-cancel the merge before renaming"})
			cmds = append(cmds, cmd)
			break
		}
		if m.view.IsUntitled() {
			// First materialize of an untitled doc (bind-new). Do NOT bind the view
			// to a path yet: store.Materialize's RenameExcl create path refuses to
			// clobber an existing file (no-clobber, step 6), and binding
			// optimistically would let a later ⌘S overwrite it (Catastrophic,
			// rung 1). The view stays untitled until the write succeeds;
			// FileSavedMsg{BindNew} performs the bind then.
			if m.store == nil {
				m.footer, cmd = m.footer.Update(footer.ShowErrorMsg{Text: "cannot save: storage not ready"})
				cmds = append(cmds, cmd)
				break
			}
			newPath := filepath.Join(m.currentDir(), msg.Name+".md")
			requestID := fmt.Sprintf("bind-%v", time.Now().UnixNano())
			m.activeSave = SaveIdentity{RequestID: requestID, SavedContent: []byte(m.editor.Content()), InFlight: true, Path: newPath, DocID: m.view.DocID()}
			// bind-new: RenameExcl's target-existence check is the guard; expect=0
			// is safe (Materialize's create path never consults it).
			seq := m.currentSeqFor(m.view.DocID())
			cmds = append(cmds, materializeStoreCmd(m.store, m.view.DocID(), newPath, m.editor.Content(), 0, seq, requestID, true))
		} else {
			dir := filepath.Dir(m.view.Path())
			newPath := filepath.Join(dir, msg.Name+".md")
			cmds = append(cmds, fileRenameCmd(m.fsys(), m.view.Path(), newPath))
		}

	case filetree.FileSelectedMsg:
		m, cmd = m.requestOpenPath(0, msg.Path)
		cmds = append(cmds, cmd)

	case filetree.FileDeleteRequestedMsg:
		// §1.4.4 — block trash of the active dirty document without prompting.
		// executeClose skips the dirty guard; refusing here keeps the unsaved buffer safe.
		if msg.Path == m.view.Path() && m.isViewDirty() {
			m.footer, cmd = m.footer.Update(footer.ShowErrorMsg{Text: "Unsaved changes — save (⌘S) or close first"})
			cmds = append(cmds, cmd)
			break
		}
		// Raise a confirmation guard before acting — §1.4.4.
		m.pendingDataLoss = pendingDataLoss{kind: actionTrash, pendingTrashPath: msg.Path}
		m.footer = m.footer.SetGuard(footer.GuardTrash, trashGuardOptions)

	case FileDeletedMsg:
		// Trash succeeded — optimistic removal was correct.
		if m.view.Path() == msg.Path {
			// Active doc deleted: close its tab and open the neighbour (or untitled).
			m, cmd = m.executeClose(m.view.DocID(), m.view.Path())
			cmds = append(cmds, cmd)
		} else {
			// Background tab — remove without touching the active view.
			m.opentabs = m.opentabs.CloseFile(msg.Path)
		}

	case FileDeleteErrorMsg:
		// Trash failed — restore the view to real disk state and surface the error.
		cmds = append(cmds, reloadDirCmd(m.fsys(), m.filetree.Root()))
		m.footer, cmd = m.footer.Update(footer.ShowErrorMsg{Text: msg.Err.Error()})
		cmds = append(cmds, cmd)

	case filetree.DirSelectedMsg:
		cmds = append(cmds, loadDirCmd(m.fsys(), msg.Path))

	case filetree.DirLoadedMsg:
		// A successful load resolves any prior sticky top-banner error (focus-
		// trap fix) — the dir is readable again.
		m.err = nil
		// The editor resolves links/embeds from the open document's own path +
		// the static workspace root (set once at New) — no per-dir SetDir.
		m.breadcrumb = m.breadcrumb.SetDir(msg.Root)
		m, cmd = m.startWatch(msg.Root)
		cmds = append(cmds, cmd)

	case filetree.DirReloadedMsg:
		// A successful disk-triggered reload resolves any prior sticky
		// top-banner error (focus-trap fix) — e.g. the watched dir came back.
		m.err = nil

	case dirChangedMsg:
		dir := m.watchedDir
		cmds = append(cmds, reloadDirCmd(m.fsys(), dir))
		m, cmd = m.startWatch(dir)
		cmds = append(cmds, cmd)
		// BUG1/Fix C/§1.4.7: dirChangedMsg is the umbrella event for every dir
		// change fsnotify reports (Write/Create/Remove/Rename) — an ATOMIC-SAVE
		// external editor (temp→rename, the common case: vim/VS Code/most
		// tools) lands as Create/Rename, which fileChangedMsg (the in-place-
		// Write path below) never sees. One async Probe of the current doc
		// covers both the deletion guard AND the passive "changed on disk"
		// hint uniformly (handleProbeResult) — no separate currentFileMissing
		// stat call needed before it.
		cmds = append(cmds, probeDocCmd(m.store, m.view.DocID(), m.view.Path()))

	case fileChangedMsg:
		// BUG1: fsnotify saw an in-place Write to a file in the watched dir. If
		// it's the open file, probe for divergence (handleProbeResult surfaces
		// the passive "changed on disk" hint — no modal). Re-arm the one-shot
		// watcher.
		if msg.path == m.view.Path() {
			cmds = append(cmds, probeDocCmd(m.store, m.view.DocID(), m.view.Path()))
		}
		m, cmd = m.startWatch(m.watchedDir)
		cmds = append(cmds, cmd)

	case opentabs.TabSelectedMsg:
		// WP2 item 1: a data-loss guard owns navigation until resolved (§1.4.4),
		// mirroring handleKeyPress's InGuard early-return — otherwise a tab
		// select delivered while e.g. GuardMerge is up (raised via mouse click
		// on the tab bar, or a stray Enter) could switch the view out from
		// under an in-flight conflict resolution.
		if m.footer.InGuard() {
			break
		}
		// G: stat-on-focus is now free — requestOpenPath always routes a file
		// switch through store.Load (even for a previously-visited tab), and
		// Load's own SyncState IS the freshness check (handleFileLoadedMsg
		// derives the passive "changed on disk" hint from it directly). No
		// separate probe needed here.
		m, cmd = m.requestOpenPath(msg.DocID, msg.Path)
		cmds = append(cmds, cmd)

	case markdownedit.ImageSaveErrorMsg:
		// B2: markdownedit.ImageSaveErrorMsg's own Update handler is a no-op
		// (there is no other handler tree-wide) — an image-write failure
		// (commands_image.go) or a ReplaceRange failure routed there would
		// otherwise surface nowhere. The workspace, as the message's single
		// consumer, surfaces it here via the existing errorCmd chokepoint; the
		// message itself stays defined in markdownedit, its producer (§2.4).
		cmds = append(cmds, errorCmd(msg.Err))

	case markdownedit.LinkActivatedMsg:
		// The editor already resolved the target (one resolver, existence-checked);
		// we just branch on the discriminant (§1.7).
		switch msg.Kind {
		case markdownedit.LinkExternal:
			// http(s)/mailto → OS default handler (async Cmd).
			cmds = append(cmds, openExternalCmd(msg.Dest))
		case markdownedit.LinkInternal:
			// Dest exists (resolver confirmed it) → open + switch; previous file
			// stays a tab. No strand vector: a missing target is LinkMissing below,
			// so requestOpenPath never blanks the editor for a dead link (§0).
			m, cmd = m.requestOpenPath(0, msg.Dest)
			cmds = append(cmds, cmd)
			m.footer, cmd = m.footer.Update(footer.ShowStatusMsg{Text: "→ " + filepath.Base(msg.Dest)})
			cmds = append(cmds, cmd)
		case markdownedit.LinkMissing:
			m.footer, cmd = m.footer.Update(footer.ShowErrorMsg{Text: "Link not found: " + msg.Raw})
			cmds = append(cmds, cmd)
		}

	case FileLoadedMsg:
		// See workspace_io_handlers.go: handleFileLoadedMsg.
		m, cmds = m.handleFileLoadedMsg(msg, cmds)

	case FileRenamedMsg:
		// ok ignored: this is a user-initiated rename via the title field,
		// which already validates the new name against collisions
		// (title.Commit/validateFileName) before this message is ever
		// produced — a refusal here would mean that validation has a gap,
		// not something this call site should paper over.
		m.opentabs, _ = m.opentabs.RenameFile(msg.OldPath, msg.NewPath)
		// Rebind the VFS doc to the new path. os.Rename preserved the inode, so
		// OpenPath finds the same doc and just updates its path — preserving the
		// undo history. We initiated this rename, so the RenamedFrom warning is
		// expected and ignored.
		renamedDocID := m.view.DocID()
		if m.store != nil {
			if ref, err := m.store.OpenPath(msg.NewPath); err == nil {
				renamedDocID = ref.ID
			}
		}
		m.view = fileView(msg.NewPath, renamedDocID)
		m.breadcrumb = m.breadcrumb.SetPath(msg.NewPath)
		m.title = m.title.SetText(strings.TrimSuffix(filepath.Base(msg.NewPath), ".md"))
		if root := m.filetree.Root(); root != "" {
			cmds = append(cmds, reloadDirCmd(m.fsys(), root))
		}

	case FileRenameErrorMsg:
		m.footer, cmd = m.footer.Update(footer.ShowErrorMsg{Text: msg.Err.Error()})
		cmds = append(cmds, cmd)

	case FileSavedMsg:
		// See workspace_io_handlers.go: handleFileSavedMsg.
		var saveHandled bool
		m, cmds, saveHandled = m.handleFileSavedMsg(msg, cmds)
		if saveHandled {
			return m, tea.Batch(cmds...) // quit: bypasses broadcast section
		}

	case probeResultMsg:
		// Passive "changed on disk" hint / GuardDeleted trigger (dirChangedMsg,
		// fileChangedMsg, and the flush tick).
		m, cmd = m.handleProbeResult(msg)
		cmds = append(cmds, cmd)

	case resolveProbeMsg:
		// Fix A: async result of the FRESH probe launched by [M]/[D].
		var mergeCmd tea.Cmd
		m, mergeCmd = m.handleResolveProbe(msg)
		cmds = append(cmds, mergeCmd)

	case unwindProbeMsg:
		// Fix B: async result of the FRESH probe launched when an
		// undo/redo unwinds the merge resolver active→inactive.
		var unwindCmd tea.Cmd
		m, unwindCmd = m.handleUnwindProbe(msg)
		cmds = append(cmds, unwindCmd)

	case FileSaveErrorMsg:
		// See workspace_io_handlers.go: handleFileSaveErrorMsg.
		m, cmds = m.handleFileSaveErrorMsg(msg, cmds)

	case FileLoadErrorMsg:
		// A broken link or vanished file. Drop a stale failure (gen mismatch) — the
		// user already moved past that read. For the awaited read, lift the gate and
		// surface the error; the buffer + identity were never touched, so the
		// previous, fully-consistent document is preserved — no stranding, and ⌘S
		// can't write blank over a real file (§1.3 / §1.4).
		if !m.pendingLoad.active || msg.Gen != m.pendingLoad.gen {
			break
		}
		m.pendingLoad = pendingLoad{}
		m.footer, cmd = m.footer.Update(footer.ShowErrorMsg{Text: msg.Err.Error()})
		cmds = append(cmds, cmd)

	case fileWatchReadError:
		m.err = fmt.Errorf("external change to %s: %w", msg.path, msg.err)

	case StoreReadyMsg:
		// See workspace_io_handlers.go: handleStoreReadyMsg.
		m, cmds = m.handleStoreReadyMsg(msg, cmds)

	case lateBindLoadedMsg:
		// Identity resolution for a file that opened before the store was
		// ready (handleStoreReadyMsg's async follow-up). See
		// workspace_io_handlers.go: handleLateBindLoaded.
		m, cmd = m.handleLateBindLoaded(msg)
		cmds = append(cmds, cmd)

	case pendingFlushMsg:
		// Only the most recent flush request fires a snapshot (debounce). Earlier
		// goroutines return stale gen values and are dropped here.
		if msg.gen == m.flushGen && m.store != nil && m.view.DocID() > 0 {
			content := m.editor.Content()
			// Capture seq synchronously, co-atomic with content, so the snapshot
			// is tagged at the exact journal position the content reflects.
			// Never read CurrentSeq inside the goroutine — later AppendEdits on
			// the main loop would advance the head, tagging old content with a
			// newer seq and corrupting RecoverDocument (plan §C, CRITIC #3).
			if seq, err := m.store.CurrentSeq(m.view.DocID()); err == nil {
				cmds = append(cmds, snapshotCmd(m.store, m.view.DocID(), content, uint64(seq), msg.gen))
			}
			// On error: skip the snapshot rather than mistag at seq 0.
			// fire-and-forget: the journal stays durable (§1.4.3).
		}
		// G: probe-on-flush-tick — periodically re-check the current doc's
		// disk state even with no focus change (handleProbeResult, workspace_probe.go).
		cmds = append(cmds, probeDocCmd(m.store, m.view.DocID(), m.view.Path()))

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
		// WP2 item 1: mirror handleKeyPress's InGuard early-return (§1.4.4) — a
		// guard prompt owns the mouse too, so a click can never switch panes/
		// tabs out from under an in-flight conflict resolution.
		if m.footer.InGuard() {
			return m.finalize(cmds)
		}
		return m.handleMouseClick(msg, cmds)

	case tea.MouseMotionMsg:
		return m.handleMouseMotion(msg, cmds)

	case footer.ConfirmQuitMsg:
		// Modal merge (§4): refuse quitting while the active doc is unresolved —
		// saveAllDirtyForQuit would otherwise materialize its marker buffer to
		// disk (it is dirty by construction). Esc-abort is the escape hatch.
		if m.HasUnresolvedConflicts() {
			m.footer, cmd = m.footer.Update(footer.ShowErrorMsg{Text: "Resolve or Esc-cancel the merge before quitting"})
			return m, cmd
		}
		if m.anyDirty() { // ground-truth (H3, §1.4.8) — never the cached opentabs flag alone
			m.pendingDataLoss = pendingDataLoss{kind: actionQuit}
			m.footer = m.footer.SetGuard(footer.GuardDirty, dataLossGuardOptions)
			return m, nil
		}
		return m.teardownAndQuit()

	case footer.DataLossGuardResponseMsg:
		// See workspace_conflict.go: handleDataLossGuardResponse.
		var guardDone bool
		m, cmds, guardDone = m.handleDataLossGuardResponse(msg, cmds)
		if guardDone {
			return m, tea.Batch(cmds...) // quit: bypasses broadcast section
		}

	case footer.DictationStartMsg:
		// Part IV: tag the session with the CURRENT view ticket for whichever
		// target it's anchored to (paneChat's chatDocID is stable — no epoch
		// churn of its own, so epoch stays 0 and is never checked for it).
		var ticketDocID int64
		var ticketEpoch uint64
		if m.focus == paneChat {
			ticketDocID = m.chatDocID
		} else {
			ticketDocID = m.view.DocID()
			ticketEpoch = m.epoch
		}
		m.dict = m.dict.Enable(m.editor.CursorOffset(), ticketDocID, ticketEpoch)
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
		m = m.setFocus(paneCenter)
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

		// A content-bearing broadcast (paste/clipboard, async image-insert) mutates
		// the editor buffer, so it MUST drain+journal in THIS pass — exactly like the
		// keypress path (workspace_update_keys.go paneCenter). Otherwise the edit is
		// left un-drained and a later keystroke journals it, advancing the journal
		// past the saved position and flipping dirty with no content change (§1.4.8).
		// Non-mutating broadcasts (timers, image ticks, resize) drain empty, so
		// journalEdit is a no-op. Gated on the focused editor, mirroring the key path
		// EXACTLY — including while a dictation session is enabled: the key path
		// journals unconditionally, and an enabled-dictation gate here made a paste
		// during dictation apply to the buffer but never reach the journal (buffer
		// ahead of journal — unsaved work invisible to crash recovery, §1.4.2/§1.4.8;
		// found via FuzzSession once ^V became reachable). Dictation's own edits
		// can't double-journal through this drain: routeDictationEdit applies and
		// drains them with its ticket before this section ever runs.
		prevEditorCursors := m.editor.Cursors()
		m.editor, cmd = m.editor.Update(msg)
		cmds = append(cmds, cmd)
		if m.focus == paneCenter {
			var editorEdits []buffer.AppliedEdit
			m.editor, editorEdits = m.editor.DrainEdits()
			m = m.journalEdit("main", editorEdits, prevEditorCursors, m.editor.Cursors(), &cmds)
		}

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
