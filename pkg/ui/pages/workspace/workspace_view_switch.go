package workspace

import (
	"fmt"
	"path/filepath"
	"strings"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/ui/components/opentabs"
	"rune/pkg/ui/help"
)

// showHelp loads the read-only help document into the shared editor.
// Synchronous: the content is generated in memory, no I/O deferred.
func (m Model) showHelp() (Model, tea.Cmd) {
	var cmd tea.Cmd
	m.editor, cmd = m.editor.SetContent(m.helpContent)
	m.editor = m.editor.SetReadOnly(true)
	m = m.bumpEpoch() // Part IV: a help-switch buffer install invalidates every outstanding view ticket
	m.view = helpView()
	m.title = m.title.SetText("(Help)")
	m.breadcrumb = m.breadcrumb.SetPath("")
	m.opentabs = m.opentabs.OpenFile(0, help.DocPath)
	m.opentabs = m.opentabs.SetName(opentabs.TabHandle{Path: help.DocPath}, "(Help)")
	m.opentabs = m.opentabs.SetDirty(opentabs.TabHandle{Path: help.DocPath}, false)
	m = m.setFocus(paneCenter)
	return m, cmd
}

// showUntitled switches to the untitled document with the given docID,
// reconstructing its content from the VFS (crash-safe — a "recovery install",
// Part IV). All untitled tabs share path ""; the docID is the only stable key
// (N4). RecoverAcrossSessions (v10, B2) distinguishes a brand-new scratch (no
// record anywhere → empty) from one whose content was deleted, AND from a
// docID this session has never itself touched — the ordinary case for any
// tab restoreScratch surfaced this launch — recovering a DIFFERENT, now-
// confirmed-dead session's draft in that case rather than silently reading
// this brand-new session's own (trivially empty) reconstruction.
func (m Model) showUntitled(docID int64) (Model, tea.Cmd) {
	content := ""
	if docID > 0 && m.store != nil {
		if c, found, err := m.store.RecoverAcrossSessions(docID); err == nil && found {
			content = c
		}
	}
	var cmd tea.Cmd
	m.editor, cmd = m.editor.SetContent(content)
	m.editor = m.editor.SetReadOnly(false)
	m = m.bumpEpoch() // Part IV: an untitled/recovery buffer install invalidates every outstanding view ticket
	m.view = untitledView(docID)
	if name := m.opentabs.NameOf(opentabs.TabHandle{DocID: docID}); name != "" {
		m.title = m.title.SetText(name)
	}
	m.breadcrumb = m.breadcrumb.SetPath("")
	m.opentabs = m.opentabs.OpenFile(docID, "")
	m = m.setFocus(paneCenter)
	return m, cmd
}

// ensureScratchDoc gives the current store-less untitled buffer a durable VFS
// document once the store is available. The startup untitled is created before
// the store opens (docID==0); without this its edits would never be journaled
// and a crash would lose the whole session (§1.4.3). Any content typed before
// the store was ready is snapshotted so it is recoverable.
func (m Model) ensureScratchDoc() Model {
	if m.store == nil || !m.view.IsUntitled() || m.view.DocID() != 0 {
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
	m.view = m.view.withDocID(ref.ID)
	m.opentabs = m.opentabs.AssignDocID("", ref.ID)
	if content := m.editor.Content(); content != "" {
		if _, err := m.store.CreateSnapshot(ref.ID, content, 0); err != nil {
			_ = err // fire-and-forget: best-effort; subsequent edits journal normally
		}
	}
	return m
}

// bindMaterialized reflects the bind-new identity transition in workspace UI
// state, for the doc identified by oldDocID (the untitled doc captured at
// save-start — see SaveIdentity — NOT necessarily m.view's current docID). The
// VFS-side identity rebind (documents.path/inode/device/kind) was already
// committed inside store.Materialize's own commit tx (Part III step 5) — this
// only updates the workspace's own tab/view/breadcrumb/title bookkeeping, so
// the undo history built while untitled survives the bind (§1.4.6) without a
// second, redundant Bind call here.
//
// The tab-bar entry always updates — a background tab's bind-new must be
// reflected there even if the user has since switched away. stillDisplayed
// additionally gates the DISPLAYED view/breadcrumb/title: those must change only
// when oldDocID is still what m.view shows, otherwise this ack would stamp the
// wrong tab's identity onto whatever the user has since navigated to (the same
// corruption class the overwrite-save path was fixed for in handleFileSavedMsg).
func (m Model) bindMaterialized(oldDocID int64, path string, stillDisplayed bool) Model {
	docID := oldDocID
	if docID == 0 && m.store != nil {
		// Defensive fallback: bind-new always saves a real, already-scratch-
		// created doc, so this should not happen — but never silently drop the
		// file if it somehow does; resolve identity via the just-written path.
		if ref, err := m.store.OpenPath(path); err == nil {
			docID = ref.ID
		}
	}
	if oldDocID != 0 {
		m.opentabs = m.opentabs.OpenFile(docID, path)
	} else {
		// ok ignored: a refusal (path already a DIFFERENT open tab) leaves
		// the bound tab showing "" (still looks untitled in the tab bar) —
		// cosmetic only; the file itself is already durably written via
		// Materialize by the time this bookkeeping runs (§0 — no data risk).
		m.opentabs, _ = m.opentabs.RenameFile("", path)
		if docID != 0 {
			m.opentabs = m.opentabs.AssignDocID(path, docID)
		}
	}
	if stillDisplayed {
		m.view = fileView(path, docID)
		m.breadcrumb = m.breadcrumb.SetPath(path)
		m.title = m.title.SetText(strings.TrimSuffix(filepath.Base(path), ".md"))
	}
	return m
}

// restoreScratch surfaces genuine, NON-EMPTY unsaved untitled documents left in
// the VFS by a prior session as recoverable tabs, then garbage-collects empty
// scratch rows so the store does not grow unbounded (Decision 2).
//
// Two filters keep this from resurrecting junk: RecoverableScratch already
// excludes orphaned bound-doc rows (inode IS NOT NULL, §1.7 — D7: this used
// to say "inode != 0", the pre-v8 in-band sentinel the identity columns no
// longer use); here we reconstruct each candidate — via RecoverAcrossSessions
// (v10, B2), since this is always a BRAND NEW session for every id
// RecoverableScratch returns (a raw HasHistory+RecoverDocument pair would
// silently read this session's own trivially-empty reconstruction of a docID
// it has never itself touched — the exact B2 defect) — and skip any whose
// content is empty/whitespace-only (or whose owning session is still alive:
// RecoverAcrossSessions reports found=false, and its private draft correctly
// stays private), so a blank or still-in-use scratch never reopens as a tab.
// Best-effort — failures never block startup; content loads lazily when the
// user selects the tab.
func (m Model) restoreScratch() Model {
	if m.store == nil {
		return m
	}
	if ids, err := m.store.RecoverableScratch(m.view.DocID()); err == nil {
		for _, id := range ids {
			content, found, err := m.store.RecoverAcrossSessions(id)
			if err != nil || !found || strings.TrimSpace(content) == "" {
				continue // skip empty/unrecoverable/still-live scratches — recover non-empty dead work only
			}
			name := m.nextUntitledName()
			m.opentabs = m.opentabs.OpenFile(id, "")
			m.opentabs = m.opentabs.SetName(opentabs.TabHandle{DocID: id}, name)
		}
		// Active state is restored by finalize() → SetActive(m.docID) after this returns.
	}
	if _, err := m.store.GCEmptyScratch(m.view.DocID()); err != nil {
		_ = err // fire-and-forget: housekeeping; non-fatal
	}
	return m
}
