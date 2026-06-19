package workspace

import (
	"context"
	"fmt"
	"os"
	"path/filepath"

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

func (m Model) requestOpenPath(path string) (Model, tea.Cmd) {
	if path == m.filePath {
		return m, nil
	}
	// Preserve the untitled buffer before leaving so switching back restores it.
	if m.filePath == "" {
		m.untitledContent = m.editor.Content()
		m.untitledTitle = m.title.Text()
	}
	if path == help.DocPath {
		return m.showHelp(), nil
	}
	if path == "" {
		return m.showUntitled(), nil
	}
	return m, loadFileCmd(context.Background(), path)
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
	return m.requestOpenPath(help.DocPath)
}

// showHelp loads the read-only help document into the shared editor.
// Synchronous: the content is generated in memory, no I/O deferred.
func (m Model) showHelp() Model {
	m.editor = m.editor.SetContent(m.helpContent).SetReadOnly(true)
	m.filePath = help.DocPath
	m.docID = 0
	m.headSeq = 0
	m.cleanRev = m.editor.Revision()
	m.title = m.title.SetText("(Help)")
	m.breadcrumb = m.breadcrumb.SetPath("")
	m.opentabs = m.opentabs.OpenFile(0, help.DocPath)
	m.opentabs = m.opentabs.SetTabName(help.DocPath, "(Help)")
	m.opentabs = m.opentabs.MarkClean(help.DocPath)
	m.focus = paneCenter
	return m
}

// showUntitled restores the untitled buffer when switching back to the "" tab.
// Prefers VFS reconstruction (crash-safe) over the in-memory RAM stash.
func (m Model) showUntitled() Model {
	// Locate the untitled tab's docID from opentabs cursor (cursor is already
	// on the untitled tab when this is called from the TabSelectedMsg handler).
	docID := m.opentabs.DocIDAt(m.opentabs.Cursor())

	content := m.untitledContent // RAM stash fallback
	if docID > 0 && m.store != nil {
		if vfsContent, err := m.store.RecoverDocument(docID); err == nil && vfsContent != "" {
			content = vfsContent
		}
	}

	m.editor = m.editor.SetContent(content).SetReadOnly(false)
	m.filePath = ""
	m.docID = docID
	m.headSeq = 0
	if m.untitledTitle != "" {
		m.title = m.title.SetText(m.untitledTitle)
	}
	m.breadcrumb = m.breadcrumb.SetPath("")
	m.opentabs = m.opentabs.OpenFile(docID, "")
	m.focus = paneCenter
	return m
}

// requestCloseCurrent guards against silently discarding a dirty buffer (§1.4.4).
func (m Model) requestCloseCurrent() (Model, tea.Cmd) {
	if m.editor.Revision() != m.cleanRev && !m.viewingHelp() {
		m.pendingDataLoss = dataLossActionClose
		m.footer = m.footer.SetGuard(footer.GuardDirty, quitGuardOptions)
		return m, nil
	}

	nextPath := m.opentabs.NextPath(m.filePath)
	if m.filePath == "" && nextPath == "" {
		return m, nil // sole untitled tab — keep it
	}
	return m.executeClose(m.docID, m.filePath, nextPath)
}

func (m Model) executeClose(closeDocID int64, closePath, nextPath string) (Model, tea.Cmd) {
	var cmds []tea.Cmd
	if closeDocID != 0 {
		m.opentabs = m.opentabs.CloseByID(closeDocID)
	} else {
		m.opentabs = m.opentabs.CloseFile(closePath)
	}
	if nextPath != "" {
		cmds = append(cmds, loadFileCmd(context.Background(), nextPath))
	} else {
		var createCmd tea.Cmd
		m, createCmd = m.CreateUntitled(false)
		cmds = append(cmds, createCmd)
	}
	return m, tea.Batch(cmds...)
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

// nextUntitled returns the first available "Untitled N" name in dir.
func nextUntitled(dir, skip string) string {
	for n := 1; ; n++ {
		name := fmt.Sprintf("Untitled %d", n)
		if skip != "" && name == skip {
			continue
		}
		info, err := os.Stat(filepath.Join(dir, name+".md"))
		if err != nil || info.Size() == 0 {
			return name
		}
	}
}

// CreateUntitled opens a new untitled buffer in the current filetree directory.
func (m Model) CreateUntitled(preserveCurrentUntitled bool) (Model, tea.Cmd) {
	dir := m.currentDir()
	var cmds []tea.Cmd

	skipName := ""
	if preserveCurrentUntitled && m.filePath == "" && m.editor.Content() != "" {
		skipName = m.title.Text()
		currentPath := filepath.Join(dir, skipName+".md")
		cmds = append(cmds, createFileCmd(currentPath, m.editor.Content()))
		m.opentabs = m.opentabs.RenameFile("", currentPath)
	}

	name := nextUntitled(dir, skipName)
	m.editor = m.editor.SetContent("")
	m.editor = m.editor.SetReadOnly(false)
	m.filePath = ""
	m.headSeq = 0

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

	return m, tea.Batch(cmds...)
}
