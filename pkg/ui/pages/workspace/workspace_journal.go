package workspace

import (
	"fmt"
	"time"

	tea "charm.land/bubbletea/v2"

	"rune/pkg/docstate"
	"rune/pkg/editor/buffer"
	"rune/pkg/editor/cursor"
	"rune/pkg/ui/components/footer"
)

// scheduleFlush debounces VFS autosave. It increments the generation counter
// and launches a goroutine that sleeps for flushDelay then returns
// pendingFlushMsg. The handler drops stale generations (gen != m.flushGen)
// so only the last scheduled flush fires a snapshot (N5).
func (m Model) scheduleFlush(cmds *[]tea.Cmd) Model {
	m.flushGen++
	gen := m.flushGen
	*cmds = append(*cmds, func() tea.Msg {
		time.Sleep(flushDelay)
		return pendingFlushMsg{gen: gen}
	})
	return m
}

// snapshotCmd writes a VFS snapshot for docID at the given journal seq (the
// document's current position, captured synchronously by the caller — it is NOT
// always the head, e.g. after an undo) and reports the result via
// AutosaveSettledMsg. Disk is NOT written here; that only happens on explicit ⌘S
// (§1.4.2).
func snapshotCmd(store *docstate.Store, docID int64, content string, seq, gen uint64) tea.Cmd {
	return func() tea.Msg {
		_, err := store.CreateSnapshot(docID, content, int64(seq))
		return AutosaveSettledMsg{gen: gen, err: err}
	}
}

// journalTarget selects both the journalEdit routing rule and (for
// targetMain/targetChat) which document's event stream to append to — a
// typed enum rather than a string (A1/§1.7): a typo in a string literal was
// a silent bug (an edit journaled nowhere, or against the wrong document),
// where an unknown journalTarget value is a compile error.
type journalTarget uint8

const (
	targetMain journalTarget = iota
	targetChat
	targetTitle
)

// String renders the target for error messages (fmt.Stringer).
func (t journalTarget) String() string {
	switch t {
	case targetMain:
		return "main"
	case targetChat:
		return "chat"
	case targetTitle:
		return "title"
	default:
		return "unknown"
	}
}

// journalEdit routes a drained buffer edit to the durable journal and
// schedules VFS autosave. Call after DrainEdits returns non-empty edits.
//
// target selects BOTH the routing rule and (for main/chat) which document's
// event stream to append to — I2: one document = one event stream, so there
// is no more surface column to tag the row with:
//   - targetMain → m.view.DocID() — the currently displayed document.
//   - targetChat → m.chatDocID — chat's own reserved document (previously
//     journaled against m.view.DocID() by mistake — S1/H4: chat keystrokes
//     spliced into whatever file happened to be open, and AppendEdit's
//     redo-truncation could delete chat history as "abandoned future").
//   - targetTitle → never journaled at all. Title is ephemeral rename input,
//     finalized by maybeFinalizeTitle (RenameRequestMsg) on commit — not
//     undo/redo history. This is the other half of closing S1: title
//     keystrokes can no longer splice into the file's recovered content
//     because they never reach any document's event stream. The caller
//     still drains title's own pending-edit buffer (so it doesn't grow
//     unboundedly); journalEdit just discards what it drained.
func (m Model) journalEdit(target journalTarget, edits []buffer.AppliedEdit, cursorsBefore, cursorsAfter []cursor.Cursor, cmds *[]tea.Cmd) Model {
	m, _ = m.journalEditOK(target, edits, cursorsBefore, cursorsAfter, cmds)
	return m
}

// journalEditOK is journalEdit plus an explicit "the edits are durably in
// the journal" signal, for callers whose FOLLOW-UP must not run otherwise —
// an adoption (installDiskAhead, restore-theirs) that proceeds after a
// failed/empty journal write moves the CAS baseline to content the journal
// cannot reproduce, reintroducing the F1 clobber on the error path (review
// finding). ok is false when nothing was appended: empty/title drains, no
// store, no doc, or an AppendEdit failure (buffer rolled back, error
// surfaced).
func (m Model) journalEditOK(target journalTarget, edits []buffer.AppliedEdit, cursorsBefore, cursorsAfter []cursor.Cursor, cmds *[]tea.Cmd) (Model, bool) {
	if len(edits) == 0 || target == targetTitle {
		return m, false
	}
	var docID int64
	switch target {
	case targetChat:
		docID = m.chatDocID
	default: // targetMain
		docID = m.view.DocID()
	}
	if m.store == nil || docID == 0 {
		return m, false
	}
	_, err := m.store.AppendEdit(docID, edits, cursorsBefore, cursorsAfter)
	if err != nil {
		// S2/§1.3: a failed journal write must not leave the buffer permanently
		// ahead of the (failed) journal — invert the edits just applied back
		// into the originating buffer before surfacing the error, so buffer and
		// journal never diverge (mirrors handleUndo's peek-then-commit
		// discipline). A snapshot taken afterward would otherwise tag the new
		// buffer against a stale head seq too (the B2/N5 window).
		m = m.rollbackFailedJournal(target, edits, cursorsBefore, cmds)
		capturedErr := err
		*cmds = append(*cmds, func() tea.Msg {
			return footer.ShowErrorMsg{Text: "journal write failed: " + capturedErr.Error()}
		})
		return m, false
	}
	if target == targetMain {
		m = m.scheduleFlush(cmds)
	}
	return m, true
}

// rollbackFailedJournal inverts edits (just applied to target's live buffer)
// back out of it, restoring cursorsBefore, after AppendEdit failed to record
// them durably (S2). edits/cursorsBefore are exactly what the caller passed
// to AppendEdit — the same shape handleUndo peeks from the store, so
// ApplyInverse inverts them identically. A rollback failure (the inverse
// itself doesn't fit the live buffer — should not happen since these edits
// were JUST applied) is surfaced loudly rather than panicking (§1.3): the
// buffer is left as-is, ahead of the journal, which is a Tolerable halt next
// to losing the unsaved buffer outright. target is never targetTitle here —
// journalEdit already short-circuits before ever reaching AppendEdit for it.
func (m Model) rollbackFailedJournal(target journalTarget, edits []buffer.AppliedEdit, cursorsBefore []cursor.Cursor, cmds *[]tea.Cmd) Model {
	var invErr error
	var cmd tea.Cmd
	switch target {
	case targetMain:
		m.editor, cmd, invErr = m.editor.ApplyInverse(edits)
		*cmds = append(*cmds, cmd)
		if invErr == nil {
			m.editor = m.editor.SetCursors(cursorsBefore)
		}
	case targetChat:
		m.chat, invErr = m.chat.ApplyInverse(edits)
		if invErr == nil {
			m.chat = m.chat.SetCursors(cursorsBefore)
		}
	}
	if invErr != nil {
		*cmds = append(*cmds, errorCmd(fmt.Errorf("rollback journal failure for %s: %w", target, invErr)))
	}
	return m
}
