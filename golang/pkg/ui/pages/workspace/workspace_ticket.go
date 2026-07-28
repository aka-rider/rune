package workspace

import (
	tea "charm.land/bubbletea/v2"

	"rune/pkg/ui/components/footer"
)

// viewTicket identifies the specific (document, buffer-generation) an async
// VIEW-targeted result was issued against (Part IV "the ticket + chokepoint",
// I3). docID alone is not enough: a delayed result for the CURRENTLY
// displayed doc can still be stale if something else replaced the buffer in
// the meantime (an undo, a fresh load of the same path, a merge resolve) —
// epoch (workspace_ticket.go's bumpEpoch, called at every non-journaled
// buffer transition) is what catches that "same doc, different moment" case
// docID-only staleness checks (H2's original fix) cannot.
//
// Doc-targeted results (save acks, observation records) never carry or check
// a viewTicket — they key on DocID alone and always commit, by design (the
// store's own durable record is the source of truth regardless of what the
// UI is currently displaying; see the inversion test,
// TestApplyViewResult_SaveAckCommitsAfterTabSwitch).
type viewTicket struct {
	docID int64
	epoch uint64
}

// currentTicket captures the ticket for the CURRENTLY displayed document,
// to be threaded into a view-targeted async Cmd's result message.
func (m Model) currentTicket() viewTicket {
	return viewTicket{docID: m.view.DocID(), epoch: m.epoch}
}

// bumpEpoch marks a non-journaled buffer transition: SetContent installs
// (file load, untitled switch, help switch), an undo/redo MoveUndoPos
// buffer replacement, a merge/discard resolve ReplaceAll, or a recovery
// install. Journaled edits (typing, an applied dictation chunk, a drained
// broadcast) do NOT call this — they extend the CURRENT epoch's content,
// they don't replace it wholesale. Called BEFORE the transition captures a
// FRESH ticket for anything issued after it, and invalidates every ticket
// issued before it.
func (m Model) bumpEpoch() Model {
	m.epoch++
	return m
}

// applyViewResult is the ONE gate for every view-targeted async result (Part
// IV): a load install, a theirs/merge fresh-probe application, a dictation
// chunk drain. t is the ticket captured synchronously when the async
// operation was ISSUED; if the live (docID, epoch) has since moved — a tab
// switch, an undo, a fresh load, a resolve — apply is refused and a footer
// notice is shown instead of silently applying stale content to whatever the
// user is now looking at. A doc-targeted result (a save ack, an observation
// record) never calls this — it commits by DocID alone, unconditionally.
func (m Model) applyViewResult(t viewTicket, apply func(Model) (Model, tea.Cmd)) (Model, tea.Cmd) {
	if t.docID != m.view.DocID() || t.epoch != m.epoch {
		var cmd tea.Cmd
		m.footer, cmd = m.footer.Update(footer.ShowStatusMsg{Text: "Discarded a stale background result"})
		return m, cmd
	}
	return apply(m)
}
