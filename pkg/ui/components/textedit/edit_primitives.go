package textedit

import (
	"fmt"
	"sort"

	"rune/pkg/editor/buffer"
	"rune/pkg/editor/cursor"
)

// ---- Generic programmatic-edit primitives (D15) ----

// ReplaceRange replaces the range [start, end) with text. This is the core
// primitive: insert = ReplaceRange(off,off,t), delete = ReplaceRange(s,e,"").
// The new cursor position is start+len(text) — a BYTE offset (§1.5); the text
// argument's RUNE count would misplace the cursor whenever text contains a
// multibyte rune (CJK, emoji, accents).
//
// Returns a non-nil error when the edit does not fit the live buffer
// (stale/out-of-bounds positions). Per §1.3 the failure is SURFACED, never
// swallowed: the buffer is left unchanged so the caller can halt and keep
// state coherent with it, rather than silently dropping the edit (a
// silently-dropped merge-discard/dictation/paste edit is rung-2-adjacent).
func (m Model) ReplaceRange(start, end int, text string) (Model, error) {
	if m.readOnly {
		return m, nil
	}
	if start > end {
		start, end = end, start
	}
	edit := buffer.Edit{Start: start, End: end, Insert: text}
	sorted := buffer.CloneAndSortEditsDescending([]buffer.Edit{edit})
	newBuf, applied, err := m.buf.ApplyEdits(sorted)
	if err != nil {
		return m, fmt.Errorf("replace range [%d,%d): %w", start, end, err)
	}
	m.buf = newBuf
	m.pendingEdits = append(m.pendingEdits, applied...)
	m.cursors = cursor.NewCursorSet(start + len(text))
	m.rev++
	m = m.syncDisplay()
	m = m.ScrollToCursor()
	return m, nil
}

// ApplyInverse applies the inverse of the given edits (undo).
// Does NOT accumulate into pendingEdits.
//
// Returns a non-nil error when the inverse edits do not fit the live buffer
// (stale/out-of-bounds positions). Per §1.3 the failure is SURFACED, never
// swallowed: the buffer is left unchanged so the caller can halt and keep the
// journal position coherent with it, rather than silently dropping the undo.
func (m Model) ApplyInverse(edits []buffer.AppliedEdit) (Model, error) {
	inverse := make([]buffer.Edit, len(edits))
	for i, ae := range edits {
		inverse[i] = buffer.Edit{
			Start:  ae.Start,
			End:    ae.Start + len(ae.Insert),
			Insert: ae.Deleted,
		}
	}
	inverse = buffer.CloneAndSortEditsDescending(inverse)
	newBuf, _, err := m.buf.ApplyEdits(inverse)
	if err != nil {
		return m, fmt.Errorf("apply inverse (undo): %w", err)
	}
	m.buf = newBuf
	m.rev++
	m = m.syncDisplay()
	return m, nil
}

// Reapply applies the given edits forward (redo).
// Does NOT accumulate into pendingEdits.
//
// AppliedEdit.Start values carry a baked-in cumulative shift: for coalesced
// sequential inserts (ascending) each Start is the correct cursor position in
// the running buffer after all prior inserts; for multi-cursor batches
// (descending) each Start accounts for the net displacement from lower-position
// edits in the same batch. In both cases, sorting ascending and applying
// one-at-a-time keeps each Start valid against the running buffer — the same
// invariant replayOneBatch in pkg/editor/buffer/replay.go relies on.
//
// Applies all edits to a working copy and commits only if every edit succeeds,
// leaving the buffer unchanged on any error (all-or-nothing). Per §1.3 an
// out-of-bounds or failed edit returns a non-nil error rather than a silent
// no-op: the caller MUST surface it and keep the journal position coherent with
// the (unchanged) buffer, never drop the redo silently.
func (m Model) Reapply(edits []buffer.AppliedEdit) (Model, error) {
	if len(edits) == 0 {
		return m, nil
	}
	sorted := append([]buffer.AppliedEdit(nil), edits...)
	sort.Slice(sorted, func(i, j int) bool {
		return sorted[i].Start < sorted[j].Start
	})
	work := m.buf
	for _, e := range sorted {
		start := e.Start
		end := e.Start + len(e.Deleted)
		if start < 0 || end > work.Len() || start > end {
			return m, fmt.Errorf("reapply edit out of bounds: start=%d end=%d bufLen=%d", start, end, work.Len())
		}
		newBuf, _, err := work.ApplyEdits([]buffer.Edit{{Start: start, End: end, Insert: e.Insert}})
		if err != nil {
			return m, fmt.Errorf("reapply edit [%d,%d): %w", start, end, err)
		}
		work = newBuf
	}
	m.buf = work
	m.rev++
	m = m.syncDisplay()
	return m, nil
}
