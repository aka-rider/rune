package editor

import (
	"time"

	"rune/pkg/command"
	"rune/pkg/editor/buffer"
	"rune/pkg/editor/cursor"
	"rune/pkg/editor/history"
)

// StartDictation marks the current cursor position as the start of a dictation
// session. All subsequent ApplyDictationChunk calls replace the text from this
// position forward.
func (m Model) StartDictation() Model {
	m.dictation = dictationState{
		active:   true,
		startOff: m.cursors.Primary().Position,
		totalLen: 0,
	}
	return m
}

// ApplyDictationChunk replaces the current dictation range in the buffer with
// text (the full accumulated dictation text so far, not a delta).
// No-op when dictation is not active.
func (m Model) ApplyDictationChunk(text string) Model {
	if !m.dictation.active {
		return m
	}

	newPos := m.dictation.startOff + len(text)
	primaryID := m.cursors.Primary().ID

	op := command.Operation{
		Kind: command.OperationEditBuffer,
		Edits: []buffer.Edit{{
			Start:  m.dictation.startOff,
			End:    m.dictation.startOff + m.dictation.totalLen,
			Insert: text,
		}},
		Cursors: cursor.NewCursorSetFrom([]cursor.Cursor{{
			Position: newPos,
			Anchor:   newPos,
			ID:       primaryID,
		}}),
	}

	m = m.applyOperation(op, history.EditPaste, time.Now())
	m.dictation.totalLen = len(text)
	m = m.syncDisplay()
	m = m.scrollToCursor()
	return m
}

// FinalizeDictation ends dictation mode; text already in the buffer is kept.
func (m Model) FinalizeDictation() Model {
	m.dictation = dictationState{}
	return m
}

// CancelDictation removes all dictation text from the buffer and ends mode.
func (m Model) CancelDictation() Model {
	if m.dictation.active && m.dictation.totalLen > 0 {
		newPos := m.dictation.startOff
		primaryID := m.cursors.Primary().ID
		op := command.Operation{
			Kind: command.OperationEditBuffer,
			Edits: []buffer.Edit{{
				Start:  newPos,
				End:    newPos + m.dictation.totalLen,
				Insert: "",
			}},
			Cursors: cursor.NewCursorSetFrom([]cursor.Cursor{{
				Position: newPos,
				Anchor:   newPos,
				ID:       primaryID,
			}}),
		}
		m = m.applyOperation(op, history.EditPaste, time.Now())
		m = m.syncDisplay()
	}
	m.dictation = dictationState{}
	return m
}

// IsDictating reports whether a dictation session is active.
func (m Model) IsDictating() bool { return m.dictation.active }
