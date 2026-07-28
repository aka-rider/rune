package workspace

import (
	tea "charm.land/bubbletea/v2"

	"rune/pkg/docstate"
	"rune/pkg/ui/components/footer"
)

// handleStoreReadyMsg processes a StoreReadyMsg, initialising the store and
// resolving document identity for any file that opened before the store was
// ready. Extracted (with its late-bind companions below) from
// workspace_io_handlers.go to keep that file under the 500-LoC limit
// (§1.6/§11) — this trio is the store-bootstrap seam, distinct from the
// file-I/O result handlers that remain there.
func (m Model) handleStoreReadyMsg(msg StoreReadyMsg, cmds []tea.Cmd) (Model, []tea.Cmd) {
	var cmd tea.Cmd
	m.store = msg.Store
	if msg.Warning != "" {
		m.footer, cmd = m.footer.Update(footer.ShowErrorMsg{Text: msg.Warning})
		cmds = append(cmds, cmd)
	}
	if m.store != nil {
		// Persistent banner (distinct from the transient Warning text above,
		// which auto-dismisses) for the whole session — capture-into-RAM
		// must never quietly masquerade as durability.
		m.footer = m.footer.SetDegraded(m.store.Degraded())
		// Separate persistent banner for a deliberately chosen in-memory
		// session (rootChooser's "None") — never true alongside SetDegraded
		// above, since OpenInMemory always leaves Degraded() false.
		m.footer = m.footer.SetEphemeral(m.memoryStore)
		// Reserve the stable chat sentinel doc (N7).
		if chatID, err := m.store.ReserveChatDoc(); err == nil {
			m.chatDocID = chatID
		}
		if m.view.IsFile() {
			// Bound file opened before the store was ready (docID==0):
			// resolve identity asynchronously via store.Load — it also
			// records the disk-fact observation and (on first sighting) the
			// recovery-anchor snapshot Load always stamps, WITHOUT touching
			// the live buffer (handleLateBindLoaded never calls SetContent —
			// any keystrokes typed in this race window are preserved; §1.4.3).
			cmds = append(cmds, lateBindLoadCmd(m.store, m.view.Path()))
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
	return m, cmds
}

// lateBindLoadedMsg carries the result of lateBindLoadCmd.
type lateBindLoadedMsg struct {
	path   string
	result docstate.LoadResult
	err    error
}

// lateBindLoadCmd resolves a file's VFS identity (and records its disk-fact
// observation + recovery anchor) via store.Load, for a file that was
// displayed through the pre-store-ready fallback (loadFileCmd's store==nil
// branch). Deliberately does NOT return the loaded content — the caller must
// never overwrite whatever the user may have already typed in this race
// window (§1.4.3); only identity/bookkeeping is applied.
func lateBindLoadCmd(store *docstate.Store, path string) tea.Cmd {
	return func() tea.Msg {
		result, err := store.Load(path)
		if err != nil {
			return lateBindLoadedMsg{path: path, err: err}
		}
		return lateBindLoadedMsg{path: path, result: result}
	}
}

// handleLateBindLoaded applies lateBindLoadCmd's result: identity only, never
// the buffer. Dropped if the read failed (fire-and-forget — the buffer stays
// exactly as the user sees it, safe either way) or the view already moved on
// (a different path, or identity already resolved another way).
func (m Model) handleLateBindLoaded(msg lateBindLoadedMsg) (Model, tea.Cmd) {
	if msg.err != nil || msg.path != m.view.Path() || m.view.DocID() != 0 {
		return m, nil
	}
	m.view = m.view.withDocID(msg.result.DocID)
	m.opentabs = m.opentabs.AssignDocID(msg.path, msg.result.DocID)
	return m, nil
}
