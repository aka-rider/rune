package workspace

import (
	"context"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/ui/components/footer"
	"rune/pkg/ui/help"
)

func (m Model) currentDir() string {
	if m.watchedDir != "" {
		return m.watchedDir
	}
	cwd, err := os.Getwd()
	if err != nil {
		return "."
	}
	return cwd
}

// requestOpenPath switches the editor to a document. Untitled documents all
// share path "", so they are identified by docID; bound documents and help are
// identified by path. Switching away first forces a VFS snapshot of the
// outgoing document so its latest bytes are durable (Fix 5 §4).
func (m Model) requestOpenPath(docID int64, path string) (Model, tea.Cmd) {
	switch path {
	case help.DocPath:
		if m.viewingHelp() {
			return m, nil
		}
	case "":
		if m.filePath == "" && docID != 0 && docID == m.docID {
			return m, nil
		}
	default:
		if path == m.filePath {
			return m, nil
		}
	}

	m = m.forceSnapshot()

	switch path {
	case help.DocPath:
		return m.showHelp(), nil
	case "":
		return m.showUntitled(docID), nil
	default:
		return m, loadFileCmd(m.fsys(), context.Background(), path)
	}
}

// viewingHelp reports whether the read-only help document is the active doc.
func (m Model) viewingHelp() bool { return m.filePath == help.DocPath }

// toggleHelp opens, focuses, or closes the help document, per ^?.
func (m Model) toggleHelp() (Model, tea.Cmd) {
	if m.viewingHelp() {
		if m.focus == paneCenter {
			return m.requestCloseCurrent()
		}
		m.focus = paneCenter
		return m, nil
	}
	m.opentabs = m.opentabs.OpenFile(0, help.DocPath)
	m.opentabs = m.opentabs.SetTabName(help.DocPath, "(Help)")
	return m.requestOpenPath(0, help.DocPath)
}

// showHelp loads the read-only help document into the shared editor.
// Synchronous: the content is generated in memory, no I/O deferred.
func (m Model) showHelp() Model {
	m.editor = m.editor.SetContent(m.helpContent).SetReadOnly(true)
	m.filePath = help.DocPath
	m.docID = 0
	m.baseline = diskBaseline{}
	m.title = m.title.SetText("(Help)")
	m.breadcrumb = m.breadcrumb.SetPath("")
	m.opentabs = m.opentabs.OpenFile(0, help.DocPath)
	m.opentabs = m.opentabs.SetTabName(help.DocPath, "(Help)")
	m.opentabs = m.opentabs.MarkClean(help.DocPath)
	m.focus = paneCenter
	return m
}

// showUntitled switches to the untitled document with the given docID,
// reconstructing its content from the VFS (crash-safe). All untitled tabs share
// path ""; the docID is the only stable key (N4). HasHistory distinguishes a
// brand-new scratch (no record → empty) from one whose content was deleted.
func (m Model) showUntitled(docID int64) Model {
	content := ""
	if docID > 0 && m.store != nil {
		if has, err := m.store.HasHistory(docID); err == nil && has {
			if vfs, err := m.store.RecoverDocument(docID); err == nil {
				content = vfs
			}
		}
	}
	m.editor = m.editor.SetContent(content).SetReadOnly(false)
	m.filePath = ""
	m.docID = docID
	m.baseline = diskBaseline{}
	if name := m.opentabs.NameByID(docID); name != "" {
		m.title = m.title.SetText(name)
	}
	m.breadcrumb = m.breadcrumb.SetPath("")
	m.opentabs = m.opentabs.OpenFile(docID, "")
	m.focus = paneCenter
	return m
}

// forceSnapshot writes a synchronous VFS snapshot of the current document at its
// head seq before the workspace switches away from it (Fix 5 §4), and bumps
// flushGen so any in-flight debounce flush for the outgoing doc is dropped.
// Edits are already journaled per keystroke, so this only advances the snapshot
// anchor; failure is non-fatal (the journal remains the durable record).
func (m Model) forceSnapshot() Model {
	m.flushGen++
	if m.store == nil || m.docID == 0 {
		return m
	}
	seq, err := m.store.CurrentSeq(m.docID)
	if err != nil {
		// fire-and-forget: can't position the snapshot; skip it rather than
		// mistag at seq 0. The journal is the durable record (§1.4.3).
		return m
	}
	if _, err := m.store.CreateSnapshot(m.docID, m.editor.Content(), "switch", seq); err != nil {
		_ = err // fire-and-forget: snapshot is an optimization; the journal is durable
	}
	return m
}

// ensureScratchDoc gives the current store-less untitled buffer a durable VFS
// document once the store is available. The startup untitled is created before
// the store opens (docID==0); without this its edits would never be journaled
// and a crash would lose the whole session (§1.4.3). Any content typed before
// the store was ready is snapshotted so it is recoverable.
func (m Model) ensureScratchDoc() Model {
	if m.store == nil || m.docID != 0 || m.filePath != "" || m.viewingHelp() {
		return m
	}
	if !m.opentabs.HasUntitledPlaceholder() {
		return m // launched onto a file (no startup untitled to upgrade)
	}
	ref, err := m.store.CreateScratch(m.title.Text())
	if err != nil {
		m.err = fmt.Errorf("create scratch document: %w", err)
		return m
	}
	m.docID = ref.ID
	m.opentabs = m.opentabs.AssignDocID("", ref.ID)
	if content := m.editor.Content(); content != "" {
		if _, err := m.store.CreateSnapshot(ref.ID, content, "scratch", 0); err != nil {
			_ = err // fire-and-forget: best-effort; subsequent edits journal normally
		}
	}
	return m
}

// bindMaterialized binds the current untitled doc to a file that a bind-new
// materialize just created. The VFS doc id is preserved (Store.Bind), so the
// undo history built while untitled survives the bind (§1.4.6).
func (m Model) bindMaterialized(path string) Model {
	oldDocID := m.docID
	m.filePath = path
	if m.store != nil {
		if m.docID != 0 {
			if err := m.store.Bind(m.docID, path); err != nil {
				m.err = fmt.Errorf("bind document to %q: %w", path, err)
			}
		} else if ref, err := m.store.OpenPath(path); err == nil {
			m.docID = ref.ID
		}
	}
	if oldDocID != 0 {
		m.opentabs = m.opentabs.OpenFile(m.docID, path)
	} else {
		m.opentabs = m.opentabs.RenameFile("", path)
		if m.docID != 0 {
			m.opentabs = m.opentabs.AssignDocID(path, m.docID)
		}
	}
	m.breadcrumb = m.breadcrumb.SetPath(path)
	m.title = m.title.SetText(strings.TrimSuffix(filepath.Base(path), ".md"))
	return m
}

// restoreScratch surfaces genuine, NON-EMPTY unsaved untitled documents left in
// the VFS by a prior session as recoverable tabs, then garbage-collects empty
// scratch rows so the store does not grow unbounded (Decision 2).
//
// Two filters keep this from resurrecting junk: RecoverableScratch already
// excludes orphaned bound-doc rows (inode != 0); here we reconstruct each
// candidate and skip any whose content is empty/whitespace-only, so a blank
// scratch never reopens as a tab. Best-effort — failures never block startup;
// content loads lazily when the user selects the tab.
func (m Model) restoreScratch() Model {
	if m.store == nil {
		return m
	}
	if ids, err := m.store.RecoverableScratch(m.docID); err == nil {
		for _, id := range ids {
			content, err := m.store.RecoverDocument(id)
			if err != nil || strings.TrimSpace(content) == "" {
				continue // skip empty scratches — recover non-empty work only
			}
			name := m.nextUntitledName()
			m.opentabs = m.opentabs.OpenFile(id, "")
			m.opentabs = m.opentabs.SetTabNameByID(id, name)
		}
		// Active state is restored by finalize() → SetActive(m.docID) after this returns.
	}
	if _, err := m.store.GCEmptyScratch(m.docID); err != nil {
		_ = err // fire-and-forget: housekeeping; non-fatal
	}
	return m
}

// teardownAndQuit runs the shared quit sequence: clear pending state, disable
// dictation, close the store, delete pasted images, and quit.
func (m Model) teardownAndQuit() (Model, tea.Cmd) {
	m.pendingDataLoss = pendingDataLoss{}
	m.dict = m.dict.Disable()
	if m.store != nil {
		_ = m.store.Close() // fire-and-forget: best-effort flush before quit
	}
	return m, tea.Sequence(m.editor.DeleteAllImagesCmd(), tea.Quit)
}

// saveAllDirtyForQuit materializes every dirty BOUND tab to disk before quit:
// the current tab from the editor buffer, others from their VFS reconstruction.
// Untitled dirty tabs are left untouched — durable in the VFS and recoverable
// next launch (Fix 7 §6) — so quit never blocks on a never-named doc
// (Decision 2). Teardown happens once every materialize has acked.
func (m Model) saveAllDirtyForQuit() (Model, tea.Cmd) {
	var batch []tea.Cmd
	for i, h := range m.opentabs.DirtyTabs() {
		if h.Path == "" {
			continue // untitled — nothing to write
		}
		isCurrent := (m.docID != 0 && h.DocID == m.docID) || (m.docID == 0 && h.Path == m.filePath)
		requestID := fmt.Sprintf("quitsave-%d-%d-%v", h.DocID, i, time.Now().UnixNano())
		if isCurrent {
			batch = append(batch, materializeCmd(m.fsys(), h.DocID, h.Path, m.editor.Content(), requestID, false, m.baseline))
			continue
		}
		// Non-current tab: reconstruct its bytes from the VFS. Skip (never write
		// empty/stale over a real file) if there is no store or reconstruction
		// fails — the work stays safe in the VFS.
		if m.store == nil {
			continue
		}
		content, err := m.store.Content(h.DocID)
		if err != nil {
			continue
		}
		batch = append(batch, materializeCmd(m.fsys(), h.DocID, h.Path, content, requestID, false, diskBaseline{}))
	}
	if len(batch) == 0 {
		return m.teardownAndQuit() // only untitled docs are dirty — quit now
	}
	m.pendingDataLoss = pendingDataLoss{kind: actionQuit, saveLeft: len(batch)}
	return m, tea.Batch(batch...)
}

// requestCloseCurrent guards against silently discarding a dirty buffer (§1.4.4).
func (m Model) requestCloseCurrent() (Model, tea.Cmd) {
	if !m.viewingHelp() {
		isDirty := false
		if m.store != nil && m.docID != 0 {
			if d, err := m.store.IsDirty(m.docID); err == nil {
				isDirty = d
			} else {
				isDirty = m.opentabs.HasDirty() // on error, keep buffer safe
			}
		}
		if isDirty {
			m.pendingDataLoss = pendingDataLoss{kind: actionClose}
			m.footer = m.footer.SetGuard(footer.GuardDirty, dataLossGuardOptions)
			return m, nil
		}
	}

	_, hasNext := m.opentabs.NeighborOf(m.docID, m.filePath)
	if m.filePath == "" && !hasNext {
		return m, nil // sole untitled tab — keep it
	}
	return m.executeClose(m.docID, m.filePath)
}

// executeClose removes the identified tab and switches to its neighbour (or a
// fresh untitled if none remains). The closed doc is detached first so the
// switch does not re-snapshot it.
func (m Model) executeClose(closeDocID int64, closePath string) (Model, tea.Cmd) {
	next, hasNext := m.opentabs.NeighborOf(closeDocID, closePath)

	if closeDocID != 0 {
		m.opentabs = m.opentabs.CloseByID(closeDocID)
	} else {
		m.opentabs = m.opentabs.CloseFile(closePath)
	}

	m.docID = 0
	m.filePath = ""

	if !hasNext {
		return m.CreateUntitled()
	}

	var cmd tea.Cmd
	m, cmd = m.requestOpenPath(next.DocID, next.Path)
	// For async file loads requestOpenPath issues a loadFileCmd and leaves
	// m.docID/m.filePath at 0/"". Set them optimistically so finalize() marks
	// the incoming tab active during the transition, satisfying TAB-SET.
	if m.docID == 0 && m.filePath == "" {
		m.docID = next.DocID
		m.filePath = next.Path
	}
	return m, cmd
}

// maybeFinalizeTitle validates and commits the title when focus is leaving paneTitle.
// Returns (model, cmd, ok) — if ok is false, focus change is blocked.
func (m Model) maybeFinalizeTitle() (Model, tea.Cmd, bool) {
	if m.focus != paneTitle {
		return m, nil, true
	}
	if err := validateFileName(m.title.Text()); err != nil {
		var errCmd tea.Cmd
		m.footer, errCmd = m.footer.Update(footer.ShowErrorMsg{Text: "invalid name: " + err.Error()})
		return m, errCmd, false
	}
	var renameCmd tea.Cmd
	m.title, renameCmd = m.title.Commit()
	return m, renameCmd, true
}

// nextUntitledName returns the first "Untitled N" label not already shown by an
// open tab. VFS-side only — it never touches the disk (untitled docs are VFS
// files, not disk files).
func (m Model) nextUntitledName() string {
	for n := 1; ; n++ {
		name := fmt.Sprintf("Untitled %d", n)
		if !m.opentabs.HasTabNamed(name) {
			return name
		}
	}
}

// CreateUntitled opens a fresh untitled buffer as a durable VFS document. Any
// previously-open untitled stays as its own tab and VFS doc — there is no disk
// file and nothing to preserve to disk (Fix 2 §5).
func (m Model) CreateUntitled() (Model, tea.Cmd) {
	m = m.forceSnapshot()

	m.editor = m.editor.SetContent("").SetReadOnly(false)
	m.filePath = ""
	m.baseline = diskBaseline{}

	name := m.nextUntitledName()
	var newDocID int64
	if m.store != nil {
		if ref, err := m.store.CreateScratch(name); err == nil {
			newDocID = ref.ID
		}
	}
	m.docID = newDocID

	m.title = m.title.SetText(name)
	m.breadcrumb = m.breadcrumb.SetPath("")
	m.opentabs = m.opentabs.OpenFile(newDocID, "")
	if newDocID != 0 {
		m.opentabs = m.opentabs.SetTabNameByID(newDocID, name)
	} else {
		m.opentabs = m.opentabs.SetTabName("", name)
	}
	m.focus = paneCenter

	return m, nil
}
