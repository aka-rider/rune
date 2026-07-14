package workspace

import (
	"fmt"
	"path/filepath"
	"strings"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/docstate"
	"rune/pkg/ui/components/footer"
)

// handleFileLoadedMsg processes a FileLoadedMsg, updating tab bookkeeping and
// the displayed document. Extracted from Update to keep workspace_update.go
// under the 500-LoC limit (§1.6/§11). WP5: driven entirely by
// docstate.LoadResult — Load already resolved identity, recovered content,
// and the resulting SyncState in one round trip, so there is no separate
// disk-anchor re-stamping bookkeeping here anymore.
func (m Model) handleFileLoadedMsg(msg FileLoadedMsg, cmds []tea.Cmd) (Model, []tea.Cmd) {
	// A successful load resolves any prior sticky top-banner error (focus-trap
	// fix, §ACTIVE(2)) — the filesystem just answered us, whatever was wrong
	// before is no longer blocking this read.
	m.err = nil

	// The DISPLAYED document (editor content + filePath/docID + breadcrumb/
	// title/chat) changes only if this read is the one currently awaited —
	// its Gen must still match the live pending load. A superseded or
	// out-of-order read carries a stale gen and is dropped here, so "open A,
	// see B" is impossible by construction. Capture the decision BEFORE
	// lifting the gate (lifting clears pendingLoad.active).
	applyDisplayed := m.pendingLoad.active && msg.Gen == m.pendingLoad.gen
	if applyDisplayed {
		// Settle: lift the gate now — before enforceTabLimit's early break — so a
		// tab-limit refusal shows the previous doc, never a stranded blank frame.
		m.pendingLoad = pendingLoad{}
	}

	// ---- Tab bookkeeping (UNGATED: a file that read OK is a tab regardless of
	// gen — this preserves multi-file startup and "click A then B" tab behavior).
	// finalize() re-derives the active tab from the displayed doc, so a stale
	// read's tab can never steal focus. ----
	docID := msg.Result.DocID
	if docID != 0 && msg.Result.RenamedFrom != "" {
		var noCollision bool
		m.opentabs, noCollision = m.opentabs.RenameFile(msg.Result.RenamedFrom, msg.Path)
		// Tell the user on a detected external rename (§1.4.6). Reuses
		// the same passive footer-hint mechanism as the disk-changed
		// hint and link-follow confirmations — opentabs and footer are
		// sibling components, so the page (not opentabs) is the only
		// legal place to raise this (§10).
		text := fmt.Sprintf("Renamed: %s → %s", filepath.Base(msg.Result.RenamedFrom), filepath.Base(msg.Path))
		if !noCollision {
			// newPath was ALSO an already-open, different-identity tab —
			// RenameFile detached IT (disk truth: this docID is verifiably
			// what's at newPath now) rather than leaving a duplicate path
			// (T1) or clobbering it. Its content/history/dirty state are
			// untouched — it now behaves like any other unsaved tab
			// (§1.4.4/§0: nothing discarded, no confirmation skipped).
			text = fmt.Sprintf("Renamed: %s → %s (a different open tab at %s is now unsaved — its content is preserved)",
				filepath.Base(msg.Result.RenamedFrom), filepath.Base(msg.Path), filepath.Base(msg.Path))
		}
		var renameCmd tea.Cmd
		m.footer, renameCmd = m.footer.Update(footer.ShowStatusMsg{Text: text})
		cmds = append(cmds, renameCmd)
	}
	var limitCmd tea.Cmd
	var proceed bool
	m, limitCmd, proceed = m.enforceTabLimit(docID, msg.Path)
	cmds = append(cmds, limitCmd)
	if !proceed {
		return m, cmds
	}
	m.opentabs = m.opentabs.OpenFile(docID, msg.Path)

	// ---- Displayed-document mutation (gen-gated) ----
	if !applyDisplayed {
		return m, cmds // stale read: its tab is registered above; the editor keeps the awaited doc
	}

	// Discard the empty untitled placeholder when transitioning to a real file.
	// Uses the OLD m.view (reassigned below) — must run before identity settles.
	if m.view.IsUntitled() && m.editor.Content() == "" {
		m.opentabs = m.opentabs.Close(m.view.Handle())
	}

	// Identity settles BEFORE any buffer install below (Part IV): journalEdit's
	// "main" routing reads m.view.DocID(), and the R1-adopt branch below
	// journals a real edit that must land against THIS doc's own event
	// stream — never whatever tab was still displayed when this load's async
	// result arrived (the buffer can be showing an unrelated tab right up
	// until this point).
	m = m.bumpEpoch() // Part IV: a load install invalidates every outstanding view ticket
	m.view = fileView(msg.Path, docID)
	m.breadcrumb = m.breadcrumb.SetPath(msg.Path)
	m.chat = m.chat.SetFileContext(msg.Path, msg.Result.DiskContent)
	if msg.Path != "" {
		base := strings.TrimSuffix(filepath.Base(msg.Path), ".md")
		m.title = m.title.SetText(base)
	}

	// ── Load-time conflict detection (§1.4.7 / plan step 1), driven by
	// SyncState instead of a manual ancestor/theirs/ours comparison ─────────
	// content mirrors what ends up displayed, for raiseConflictGuard's
	// oursContent argument below.
	content := msg.Result.Recovered
	switch msg.Result.Sync.Kind {
	case docstate.SyncDiskAhead:
		// R1/F1: disk changed but ours == ancestor (no unsaved local edits).
		// Install theirs as a REAL journaled adoption (installDiskAhead),
		// never a bare display-only SetContent — so store.Content(docID)
		// tracks the displayed buffer immediately, and quit/evict/a second
		// revisit can never write the stale reconstruction back over newer
		// disk.
		content = msg.Result.DiskContent
		// Clear read-only BEFORE the install: coming from the Help doc the
		// shared editor is still read-only, and textedit silently drops a
		// ReplaceAll on a read-only model — the adopt would then move the
		// CAS baseline to theirs while the buffer keeps the stale
		// reconstruction (review finding: rung-1 clobber at the next ⌘S).
		// installDiskAhead additionally refuses to adopt when the install
		// produced no edits, as defense in depth.
		m.editor = m.editor.SetReadOnly(false)
		m = m.installDiskAhead(docID, msg.Result.Recovered, content, msg.Result.Sync, &cmds)
	default:
		// Clean/BufferAhead/Diverged: display the journal reconstruction
		// (identical to DiskContent when the doc has no history yet).
		// Diverged's guard is raised below, after this install (mirrors the
		// pre-v4 ordering) — a genuine conflict is never silently adopted.
		// SetContent's Cmd IS the image discovery for this content (E1) —
		// dropping it would leave embeds unspawned until the next mutation.
		var scmd tea.Cmd
		m.editor, scmd = m.editor.SetContent(content)
		cmds = append(cmds, scmd)
	}

	m.editor = m.editor.SetReadOnly(false)

	// Passive "changed on disk" hint derives directly from Load's own
	// SyncState — no extra stat-on-focus round trip needed (WP5). SyncDiskAhead
	// is carved out: installDiskAhead just adopted it as a real journaled
	// transition, so the divergence it would otherwise warn about is ALREADY
	// fully reconciled (store.Content == buffer) by the time this hint is
	// set — showing it anyway would be stale noise the very next probe tick
	// would silently clear on its own (§1.4.8: derive from the settled
	// state, not the pre-adopt snapshot). F10: the general formula is
	// Kind==DiskAhead||Kind==Diverged (never a bare Kind!=Clean, which also
	// flags an ordinary unsaved BufferAhead edit as "changed on disk") — but
	// DiskAhead is excluded HERE specifically because installDiskAhead
	// above ALWAYS immediately reconciles it, so only Diverged is ever
	// genuinely "changed and unresolved" at this site.
	m = m.setDiskChangedHint(msg.Result.Sync.Kind == docstate.SyncDiverged)

	if msg.Result.Sync.Kind == docstate.SyncDiverged {
		var guardCmd tea.Cmd
		m, guardCmd = m.raiseConflictGuard(docID, msg.Path, content, msg.Result.Sync.Theirs.Hash, msg.Result.Sync.Theirs.Obs)
		cmds = append(cmds, guardCmd)
	}

	// WP-R4 item 6: a hardlinked file forks from its other names on disk the
	// moment we save through this path (the atomic write breaks the link).
	if msg.Result.NLink > 1 {
		var linkCmd tea.Cmd
		m.footer, linkCmd = m.footer.Update(footer.ShowStatusMsg{Text: "⚠ hardlinked file — saving breaks the link"})
		cmds = append(cmds, linkCmd)
	}

	// Load-settle is the one point the doc is guaranteed displayed with
	// settled identity: raise any raced-save guard that was queued while it
	// was in the background (evict/quit-batch races, review finding). A
	// guard already on screen (e.g. the Diverged one just raised above)
	// takes precedence — the queue entry survives for the next settle.
	if !m.footer.InGuard() {
		m = m.drainRacedQueue(docID)
	}

	// Startup should focus the first CLI file's editor once it's actually
	// loaded; ordinary interactive opens never auto-focus paneCenter here.
	if msg.Gen == 1 && len(m.initialFiles) > 0 {
		m = m.setFocus(paneCenter)
	}

	return m, cmds
}

// handleFileSavedMsg, handleFileSaveErrorMsg, and saveErrorText — the
// FileSavedMsg/FileSaveErrorMsg handling pair — live in workspace_io_save.go.

// handleStoreReadyMsg and the late-bind trio live in workspace_store_ready.go
// (extracted to keep this file under the 500-LoC limit, §1.6/§11).
