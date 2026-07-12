package workspace

import (
	"context"
	"fmt"
	"runtime"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/ui/components/footer"
	"rune/pkg/ui/help"
	"rune/pkg/ui/pages/workspace/mergemode"
)

// currentDir returns the directory new files/renames resolve against. Falls
// back to the launch-captured m.workDir (§1.4.9) rather than a runtime
// os.Getwd() — the process cwd can differ from the vault root the user
// launched rune against, and re-statting it on every rename is FS I/O this
// component doesn't need (New's own os.Getwd fallback, workspace.go, is the
// one sanctioned bootstrap call — see §1.4.9(a)).
func (m Model) currentDir() string {
	if m.watchedDir != "" {
		return m.watchedDir
	}
	if m.workDir != "" {
		return m.workDir
	}
	return "."
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
		if m.view.IsUntitled() && docID != 0 && docID == m.view.DocID() {
			return m, nil
		}
	default:
		if m.view.IsFile() && path == m.view.Path() {
			return m, nil
		}
	}

	// Modal merge (§4): a mid-merge doc must never be backgrounded — its
	// marker buffer would be snapshotted to the store, and a later quit-save or
	// evict-save could write markers to the .md with HasUnresolvedConflicts()
	// reading false for a NON-active doc. Refuse the switch; Esc-abort is the
	// escape hatch.
	if m.HasUnresolvedConflicts() {
		var cmd tea.Cmd
		m.footer, cmd = m.footer.Update(footer.ShowErrorMsg{Text: "Resolve or Esc-cancel the merge before switching files"})
		return m, cmd
	}
	// The switch is proceeding — m.merge is single, workspace-wide state that
	// pertains only to the doc being left (a FULLY-RESOLVED merge deactivates
	// without clearing its conflict list until Reset/Abort, so it could
	// otherwise linger into the next document). Clear it so it never bleeds
	// across documents; Reset is a no-op if there was nothing to clear.
	m.merge = mergemode.Reset(m.merge)

	// H1: a dictation session anchored on the outgoing document's buffer must
	// not survive the switch — its startOff/appliedLen would target whatever
	// buffer is displayed when the next chunk lands, not the one it started
	// against.
	var cmds []tea.Cmd
	m = m.disableDictationForTransition(&cmds)

	// Any new navigation intent supersedes a stale deferral (below) — mirrors
	// supersedeLoad's "most recent request wins" semantics.
	m.pendingReopen = pendingReopen{}
	if m.savingTarget(docID, path) {
		// The requested file is the exact one our own interactive save is
		// currently writing. Materialize's atomic swap may already be
		// mid-flight or complete on disk while its FileSavedMsg is still
		// in-transit — reloading now could observe the post-rename inode before
		// docstate.Bind re-stamps it, orphaning the doc's undo/snapshot history
		// onto a fresh docID (§1.4.6). Defer instead; flushPendingReopen replays
		// this once the save settles (workspace_probe.go).
		m.pendingReopen = pendingReopen{docID: docID, path: path, active: true}
		return m, tea.Batch(cmds...)
	}

	m = m.forceSnapshot()

	// This call supersedes any in-flight load — it replaces what the editor
	// shows. supersedeLoad bumps the load token (so a still-pending read fails the
	// gen guard and is dropped) and clears the gate (so a synchronous branch is
	// never masked by a blank frame); the async branch re-arms it in beginLoad.
	m = m.supersedeLoad()

	var switchCmd tea.Cmd
	switch path {
	case help.DocPath:
		m = m.showHelp()
	case "":
		m = m.showUntitled(docID)
	default:
		m, switchCmd = m.beginLoad(docID, path)
	}
	cmds = append(cmds, switchCmd)
	return m, tea.Batch(cmds...)
}

// beginLoad is the single entry point for every asynchronous file load. It arms
// the non-destructive pending-load gate (the center pane renders blank during
// the load WITHOUT destroying the buffer — preserving 16138bd's anti-flash) and
// issues the read. Interactive open, tab switch, link, eviction, close→neighbor,
// and startup all funnel through here, so load discipline lives in exactly one
// place. On any load result the buffer + identity change together, so a failed
// load is a no-op on state (§1.4: ⌘S can never write blank over a real file).
func (m Model) beginLoad(docID int64, path string) (Model, tea.Cmd) {
	m.loadGen++
	gen := m.loadGen // capture into a local before the closure (§5.5)
	m.pendingLoad = pendingLoad{gen: gen, docID: docID, path: path, active: true}
	return m, loadFileCmd(m.store, m.fsys(), context.Background(), path, gen)
}

// supersedeLoad invalidates any in-flight async load without issuing a new one: it
// bumps the load token so a still-pending read fails the gen guard (dropped, never
// displayed) and clears the gate so a synchronous transition (help/untitled/new) is
// not masked by a blank frame. The sole way to "cancel" a load from a synchronous
// transition; mirrors forceSnapshot bumping flushGen on every switch-away.
func (m Model) supersedeLoad() Model {
	m.loadGen++
	m.pendingLoad = pendingLoad{}
	return m
}

// viewingHelp reports whether the read-only help document is the active doc.
func (m Model) viewingHelp() bool { return m.view.IsHelp() }

// toggleHelp opens, focuses, or closes the help document, per ^?.
// refuseMergeTransition surfaces the modal-merge refusal hint (§4): a mid-merge
// doc must never be backgrounded/closed/renamed — its marker working buffer
// would be snapshotted to the store and a later quit/evict save could write
// markers to the .md (rung-1). Shared by every transition guard so the message
// and behavior stay consistent; Esc-abort is the escape hatch. action is the
// verb phrase completing "... before <action>".
func (m Model) refuseMergeTransition(action string) (Model, tea.Cmd) {
	var cmd tea.Cmd
	m.footer, cmd = m.footer.Update(footer.ShowErrorMsg{Text: "Resolve or Esc-cancel the merge before " + action})
	return m, cmd
}

func (m Model) toggleHelp() (Model, tea.Cmd) {
	// Modal merge (§4): opening help would background the mid-merge marker doc
	// (and the stray tab-add below would run before requestOpenPath's own gate).
	if m.HasUnresolvedConflicts() {
		return m.refuseMergeTransition("opening help")
	}
	if m.viewingHelp() {
		if m.focus == paneCenter {
			return m.requestCloseCurrent()
		}
		m = m.setFocus(paneCenter)
		return m, nil
	}
	m.opentabs = m.opentabs.OpenFile(0, help.DocPath)
	m.opentabs = m.opentabs.SetTabName(help.DocPath, "(Help)")
	return m.requestOpenPath(0, help.DocPath)
}

// showHelp and showUntitled — the requestOpenPath switch's two synchronous
// (non-async-load) targets — live in workspace_view_switch.go.

// forceSnapshot writes a synchronous VFS snapshot of the current document at its
// head seq before the workspace switches away from it (Fix 5 §4), and bumps
// flushGen so any in-flight debounce flush for the outgoing doc is dropped.
// Edits are already journaled per keystroke, so this only advances the snapshot
// anchor; failure is non-fatal (the journal remains the durable record).
func (m Model) forceSnapshot() Model {
	m.flushGen++
	if m.store == nil || m.view.DocID() == 0 {
		return m
	}
	seq, err := m.store.CurrentSeq(m.view.DocID())
	if err != nil {
		// fire-and-forget: can't position the snapshot; skip it rather than
		// mistag at seq 0. The journal is the durable record (§1.4.3).
		return m
	}
	if _, err := m.store.CreateSnapshot(m.view.DocID(), m.editor.Content(), seq); err != nil {
		_ = err // fire-and-forget: snapshot is an optimization; the journal is durable
	}
	return m
}

// ensureScratchDoc, bindMaterialized, and restoreScratch — untitled/scratch
// VFS-identity bookkeeping — live in workspace_view_switch.go.

// isViewDirty reports whether the currently displayed document has unsaved
// changes according to the docstate store. Returns false when no store or
// docID is available (safe default: assume clean, don't block).
func (m Model) isViewDirty() bool {
	if m.store == nil || m.view.DocID() == 0 {
		return false
	}
	d, err := m.store.IsDirty(m.view.DocID())
	if err != nil {
		return false
	}
	return d
}

// requestCloseCurrent guards against silently discarding a dirty buffer (§1.4.4).
func (m Model) requestCloseCurrent() (Model, tea.Cmd) {
	// Modal merge (§4): refuse closing the active doc while unresolved —
	// Esc-abort is the escape hatch.
	if m.HasUnresolvedConflicts() {
		var cmd tea.Cmd
		m.footer, cmd = m.footer.Update(footer.ShowErrorMsg{Text: "Resolve or Esc-cancel the merge before closing"})
		return m, cmd
	}
	if !m.viewingHelp() {
		isDirty := false
		if m.store != nil && m.view.DocID() != 0 {
			if d, err := m.store.IsDirty(m.view.DocID()); err == nil {
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

	_, hasNext := m.opentabs.NeighborOf(m.view.DocID(), m.view.Path())
	if m.view.IsUntitled() && !hasNext {
		return m, nil // sole untitled tab — keep it
	}
	return m.executeClose(m.view.DocID(), m.view.Path())
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

	m.view = untitledView(0)

	if !hasNext {
		return m.CreateUntitled()
	}

	// requestOpenPath leaves m.docID/m.filePath at 0/"" for an async load (the
	// save-safe transitional identity: ⌘S is inert while a load is pending, and
	// 0/"" can never be clobbered). TAB-SET — marking the incoming tab active
	// during the gap — is handled in finalize() from m.pendingLoad, NOT by
	// forging a real identity here (which would strand blank-buffer + real-path
	// on a failed neighbour load).
	return m.requestOpenPath(next.DocID, next.Path)
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

// withFinalizedTitle is the chokepoint every global-key handler in
// handleKeyPress guards on before acting: run maybeFinalizeTitle and append
// its cmd to cmds in one call, instead of the four-line
// "var ok bool; m, cmd, ok = m.maybeFinalizeTitle(); cmds = append(cmds, cmd)"
// repeated at every one of the ×9 call sites. ok=false still means "focus
// change blocked" — the caller's own `if !ok { return m.finalize(cmds) }`
// stays inline (each case's subsequent body differs, so the early return
// itself can't be centralized without a callback-shaped rewrite).
func (m Model) withFinalizedTitle(cmds []tea.Cmd) (Model, []tea.Cmd, bool) {
	var cmd tea.Cmd
	var ok bool
	m, cmd, ok = m.maybeFinalizeTitle()
	cmds = append(cmds, cmd)
	return m, cmds, ok
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

// openExternalCmd opens an external URL in the OS default handler. Only http(s)
// and mailto reach here (markdownedit.isExternalURL is the allowlist), and the
// URL is passed as a separate exec argument — never through a shell — so a
// crafted link cannot inject a command. The buffer is never touched (§1.4); the
// outcome surfaces as a transient footer status or error (§1.3).
func openExternalCmd(url string) tea.Cmd {
	return func() tea.Msg {
		name, args := externalOpener(url)
		// runOpener spawns the OS handler; DisableOpenerForTesting swaps it for a
		// no-op (workspace_open.go) so tests/fuzzing exercise the LinkExternal
		// dispatch + footer status without launching real browser/opener processes.
		if err := runOpener(name, args...); err != nil {
			return footer.ShowErrorMsg{Text: fmt.Errorf("open %q: %w", url, err).Error()}
		}
		return footer.ShowStatusMsg{Text: "→ " + url}
	}
}

// externalOpener returns the platform command and args that open url in the OS
// default handler.
func externalOpener(url string) (string, []string) {
	switch runtime.GOOS {
	case "darwin":
		return "open", []string{url}
	case "windows":
		return "rundll32", []string{"url.dll,FileProtocolHandler", url}
	default:
		return "xdg-open", []string{url}
	}
}

// CreateUntitled opens a fresh untitled buffer as a durable VFS document. Any
// previously-open untitled stays as its own tab and VFS doc — there is no disk
// file and nothing to preserve to disk (Fix 2 §5).
func (m Model) CreateUntitled() (Model, tea.Cmd) {
	m = m.forceSnapshot()
	m = m.supersedeLoad() // synchronous: drop any in-flight read so it can't display over this buffer

	m.editor = m.editor.SetContent("").SetReadOnly(false)
	m = m.bumpEpoch() // Part IV: a fresh-untitled buffer install invalidates every outstanding view ticket

	name := m.nextUntitledName()
	var newDocID int64
	if m.store != nil {
		if ref, err := m.store.CreateScratch(name); err == nil {
			newDocID = ref.ID
		}
	}
	m.view = untitledView(newDocID)

	m.title = m.title.SetText(name)
	m.breadcrumb = m.breadcrumb.SetPath("")
	m.opentabs = m.opentabs.OpenFile(newDocID, "")
	if newDocID != 0 {
		m.opentabs = m.opentabs.SetTabNameByID(newDocID, name)
	} else {
		m.opentabs = m.opentabs.SetTabName("", name)
	}
	m = m.setFocus(paneCenter)

	return m, nil
}
