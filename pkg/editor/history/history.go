package history

import (
	"time"

	"rune/pkg/editor/buffer"
	"rune/pkg/editor/cursor"
)

type EditKind int

const (
	EditInsertChar EditKind = iota
	EditDeleteChar
	EditPaste
	EditNewline
	EditMoveLine
	EditCloneLine
	EditBatch
)

type EditGroup struct {
	Edits         []buffer.AppliedEdit
	CursorsBefore []cursor.Cursor
	CursorsAfter  []cursor.Cursor
	Timestamp     time.Time
	Kind          EditKind
}

type UndoStack struct {
	groups []EditGroup
	index  int
	clock  func() time.Time
}

func New(clock func() time.Time) UndoStack {
	if clock == nil {
		clock = time.Now
	}
	return UndoStack{
		groups: nil,
		index:  -1,
		clock:  clock,
	}
}

func (s UndoStack) Push(group EditGroup) UndoStack {
	newStack := UndoStack{
		clock: s.clock,
	}
	if s.index < len(s.groups)-1 {
		newStack.groups = append([]EditGroup(nil), s.groups[:s.index+1]...)
	} else {
		newStack.groups = append([]EditGroup(nil), s.groups...)
	}
	newStack.groups = append(newStack.groups, group)
	newStack.index = len(newStack.groups) - 1
	return newStack
}

func (s UndoStack) Undo() (UndoStack, EditGroup, bool) {
	if !s.CanUndo() {
		return s, EditGroup{}, false
	}
	newStack := UndoStack{
		groups: s.groups,
		index:  s.index - 1,
		clock:  s.clock,
	}
	return newStack, s.groups[s.index], true
}

func (s UndoStack) Redo() (UndoStack, EditGroup, bool) {
	if !s.CanRedo() {
		return s, EditGroup{}, false
	}
	newStack := UndoStack{
		groups: s.groups,
		index:  s.index + 1,
		clock:  s.clock,
	}
	return newStack, s.groups[newStack.index], true
}

func (s UndoStack) CanUndo() bool {
	return s.index >= 0
}

func (s UndoStack) CanRedo() bool {
	return s.index < len(s.groups)-1
}

func (s UndoStack) ShouldCoalesce(kind EditKind, now time.Time) bool {
	if !s.CanUndo() || s.index != len(s.groups)-1 {
		return false
	}
	lastGroup := s.groups[s.index]
	if lastGroup.Kind != kind {
		return false
	}
	if kind != EditInsertChar {
		return false
	}
	if now.Sub(lastGroup.Timestamp) > 300*time.Millisecond {
		return false
	}
	
	// If the last inserted character was whitespace, it should not coalesce with new letters
	if len(lastGroup.Edits) > 0 {
		lastEdit := lastGroup.Edits[len(lastGroup.Edits)-1]
		if lastEdit.Insert == " " || lastEdit.Insert == "\t" || lastEdit.Insert == "\n" {
			return false
		}
	}
	
	return true
}

func (s UndoStack) MergeIntoLast(edits []buffer.AppliedEdit, cursorsAfter []cursor.Cursor) UndoStack {
	if !s.CanUndo() || s.index != len(s.groups)-1 {
		return s
	}

	newStack := UndoStack{
		groups: append([]EditGroup(nil), s.groups...),
		index:  s.index,
		clock:  s.clock,
	}

	grp := newStack.groups[s.index]
	
	// Create new arrays so we don't mutate the old EditGroup
	newEdits := append([]buffer.AppliedEdit(nil), grp.Edits...)
	newEdits = append(newEdits, edits...)

	newStack.groups[s.index] = EditGroup{
		Edits:         newEdits,
		CursorsBefore: grp.CursorsBefore, // kept from the very first edit in this coalesced group
		CursorsAfter:  cursorsAfter,
		Timestamp:     s.clock(), // newest edit time
		Kind:          grp.Kind,
	}

	return newStack
}

func (g EditGroup) InverseEdits() []buffer.Edit {
	var inverse []buffer.Edit
	var cumulativeDelta int

	for _, applied := range g.Edits {
		adjustedStart := applied.Start + cumulativeDelta
		inverse = append(inverse, buffer.Edit{
			Start:  adjustedStart,
			End:    adjustedStart + len(applied.Insert),
			Insert: applied.Deleted,
		})
		cumulativeDelta += len(applied.Insert) - (applied.End - applied.Start)
	}

	return inverse
}
