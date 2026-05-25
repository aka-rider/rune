package history_test

import (
	"reflect"
	
	"testing"
	"time"
	

	"rune/pkg/editor/buffer"
	"rune/pkg/editor/cursor"
	"rune/pkg/editor/history"
)

// pseudo buffer for tests
type testBuffer struct {
	text string
}

// applies edits from start to end (ascending) assuming they are non-overlapping for AppliedEdit,
// or for reverse we should apply them descending to avoid offset issues
func applyAscending(text string, edits []buffer.AppliedEdit) string {
	// we assume the edits are meant to be applied to the current text
	// Wait, applied edits are the record of what HAPPENED.
	// So we don't apply AppliedEdits to get the *new* string. The test runner will just generate the new string and the AppliedEdit.
	return text
}

func applyEdits(text string, edits []buffer.Edit) string {
	// Apply descending to avoid offsetting earlier edits
	for i := len(edits) - 1; i >= 0; i-- {
		e := edits[i]
		if e.Start < 0 || e.End > len(text) || e.Start > e.End {
			// out of bounds, should panic or something, but let's just slice bounds safe
			continue
		}
		text = text[:e.Start] + e.Insert + text[e.End:]
	}
	return text
}

func makeCursors(pos ...int) []cursor.Cursor {
	var cs []cursor.Cursor
	for _, p := range pos {
		cs = append(cs, cursor.Cursor{Position: p, Anchor: p})
	}
	return cs
}

func TestHistory_InverseEdits(t *testing.T) {
	// 3-cursor example
	// text: "let a = 1\nlet b = 2\nlet c = 3"
	// change "let" to "var"
	group := history.EditGroup{
		Edits: []buffer.AppliedEdit{
			{Start: 0, End: 3, Deleted: "let", Insert: "var"},     // first let
			{Start: 10, End: 13, Deleted: "let", Insert: "var"},   // second let
			{Start: 20, End: 23, Deleted: "let", Insert: "var"},   // third let
		},
	}

	inverse := group.InverseEdits()
	if len(inverse) != 3 {
		t.Fatalf("expected 3 inverse edits, got %d", len(inverse))
	}

	// first let
	if inverse[0].Start != 0 || inverse[0].End != 3 || inverse[0].Insert != "let" {
		t.Errorf("wrong inverse[0]: %+v", inverse[0])
	}
	// cumulative delta after first edit: len(insert) - (end-start) = 3 - (3-0) = +0
	
	// second let
	if inverse[1].Start != 10 || inverse[1].End != 13 || inverse[1].Insert != "let" {
		t.Errorf("wrong inverse[1]: %+v", inverse[1])
	}
	
	// third let
	if inverse[2].Start != 20 || inverse[2].End != 23 || inverse[2].Insert != "let" {
		t.Errorf("wrong inverse[2]: %+v", inverse[2])
	}

	// What if lengths change?
	group2 := history.EditGroup{
		Edits: []buffer.AppliedEdit{
			{Start: 0, End: 1, Deleted: "a", Insert: "apple"}, // delta = +4
			{Start: 5, End: 5, Deleted: "", Insert: "banana"}, // delta = +6
			{Start: 10, End: 12, Deleted: "cd", Insert: ""},   // delta = -2
		},
	}
	inv := group2.InverseEdits()
	if inv[0].Start != 0 || inv[0].End != 5 || inv[0].Insert != "a" {
		t.Errorf("wrong inv[0]: %+v", inv[0])
	}
	if inv[1].Start != 9 || inv[1].End != 15 || inv[1].Insert != "" {
		// adjusted start: 5 + 4 = 9
		// end: 9 + 6 = 15
		t.Errorf("wrong inv[1]: %+v", inv[1])
	}
	if inv[2].Start != 20 || inv[2].End != 20 || inv[2].Insert != "cd" {
		// cumulative = 4 + 6 = 10
		// adjusted start = 10 + 10 = 20
		// end = 20 + 0 = 20
		t.Errorf("wrong inv[2]: %+v", inv[2])
	}
}

func TestHistory_Coalescing(t *testing.T) {
	mockTime := time.Date(2024, 1, 1, 12, 0, 0, 0, time.UTC)
	s := history.New(func() time.Time { return mockTime })

	// push first insert
	s = s.Push(history.EditGroup{
		Kind: history.EditInsertChar,
		Timestamp: mockTime,
	})

	// Rule 1: < 300ms coalesce
	mockTime = mockTime.Add(299 * time.Millisecond)
	if !s.ShouldCoalesce(history.EditInsertChar, mockTime) {
		t.Errorf("should coalesce at 299ms")
	}

	// Rule 6: > 300ms split
	mockTime = mockTime.Add(2 * time.Millisecond) // total 301ms from start
	if s.ShouldCoalesce(history.EditInsertChar, mockTime) {
		t.Errorf("should NOT coalesce at 301ms")
	}

	// Rule 3, 4, 5: deletion, paste, newline never coalesces
	if s.ShouldCoalesce(history.EditDeleteChar, mockTime) {
		t.Errorf("delete should never coalesce")
	}
	if s.ShouldCoalesce(history.EditPaste, mockTime) {
		t.Errorf("paste should never coalesce")
	}
	if s.ShouldCoalesce(history.EditNewline, mockTime) {
		t.Errorf("newline should never coalesce")
	}
}

func TestHistory_MergeIntoLast(t *testing.T) {
	mockTime := time.Date(2024, 1, 1, 12, 0, 0, 0, time.UTC)
	s := history.New(func() time.Time { return mockTime })

	s = s.Push(history.EditGroup{
		Edits: []buffer.AppliedEdit{{Start: 0, End: 0, Insert: "a"}},
		CursorsBefore: makeCursors(0),
		CursorsAfter: makeCursors(1),
		Timestamp: mockTime,
		Kind: history.EditInsertChar,
	})

	mockTime = mockTime.Add(250 * time.Millisecond)
	s = s.MergeIntoLast(
		[]buffer.AppliedEdit{{Start: 1, End: 1, Insert: "b"}},
		makeCursors(2),
	)

	_, grp, ok := s.Undo()
	if !ok {
		t.Fatalf("undo failed")
	}
	
	if len(grp.Edits) != 2 {
		t.Errorf("expected 2 merged edits")
	}
	if !reflect.DeepEqual(grp.CursorsBefore, makeCursors(0)) {
		t.Errorf("cursors before not preserved")
	}
	if !reflect.DeepEqual(grp.CursorsAfter, makeCursors(2)) {
		t.Errorf("cursors after not updated")
	}
	if !grp.Timestamp.Equal(mockTime) {
		t.Errorf("timestamp not updated to latest time: got %v, expected %v", grp.Timestamp, mockTime)
	}

	// Continuous typing advancing timestamp test (Rule 7)
	// If timestamp advanced, another 250ms jump is 500ms from start but still < 300ms from last merge
	mockTime = mockTime.Add(250 * time.Millisecond)
	// Needs to check if CanUndo/coalesce would work with s (before we called undo)
	s = s.Push(grp) // put back
	if !s.ShouldCoalesce(history.EditInsertChar, mockTime) {
		t.Errorf("should coalesce after continuous typing because timestamp advanced")
	}
}

func TestHistory_UndoRedo_Property(t *testing.T) {
	s := history.New(time.Now)

	var historyStates []struct{
		text string
		cur []cursor.Cursor
	}
	
	text := ""
	cur := makeCursors(0)
	historyStates = append(historyStates, struct{text string; cur []cursor.Cursor}{text, cur})

	// operations
	ops := []struct{
		pos int
		del string
		ins string
	}{
		{0, "", "hello"},
		{5, "", " world"},
		{5, " ", ""},
		{5, "", ","},
	}

	for _, op := range ops {
		beforeCur := cur
		afterCur := makeCursors(op.pos + len(op.ins))

		e := buffer.AppliedEdit{
			Start: op.pos, End: op.pos + len(op.del),
			Deleted: op.del, Insert: op.ins,
		}

		group := history.EditGroup{
			Edits: []buffer.AppliedEdit{e},
			CursorsBefore: beforeCur,
			CursorsAfter: afterCur,
		}

		s = s.Push(group)
		
		text = text[:op.pos] + op.ins + text[op.pos+len(op.del):]
		cur = afterCur
		
		historyStates = append(historyStates, struct{text string; cur []cursor.Cursor}{text, cur})
	}

	// Check P3 (N pushes + N undos = original)
	for i := len(historyStates) - 1; i > 0; i-- {
		prevState := historyStates[i-1]
		
		var grp history.EditGroup
		var ok bool
		s, grp, ok = s.Undo()
		if !ok {
			t.Fatalf("undo %d failed", i)
		}

		// Apply InverseEdits
		inv := grp.InverseEdits()
		text = applyEdits(text, inv)
		
		if text != prevState.text {
			t.Errorf("undo %d text mismatch: got %q, want %q", i, text, prevState.text)
		}
		if !reflect.DeepEqual(grp.CursorsBefore, prevState.cur) {
			t.Errorf("undo %d cursor mismatch", i)
		}
	}

	// Check False undo
	if _, _, ok := s.Undo(); ok {
		t.Errorf("expected undo to fail when empty")
	}

	// Check P4 (Undo + Redo = restores exact state)
	for i := 1; i < len(historyStates); i++ {
		nextState := historyStates[i]
		
		var grp history.EditGroup
		var ok bool
		s, grp, ok = s.Redo()
		if !ok {
			t.Fatalf("redo %d failed", i)
		}
		
		// To apply redo, we convert AppliedEdit to intent Edit
		var forward []buffer.Edit
		for _, ae := range grp.Edits {
			forward = append(forward, buffer.Edit{Start: ae.Start, End: ae.End, Insert: ae.Insert})
		}
		// Redo modifies the text forward.
		// Wait, applied edits are in ascending order usually, but applyEdits expects buffer.Edit
		// If there are multiple edits in a single group, we assume they are independent or sorted.
		// For property test they are single edit groups so it's fine.
		text = applyEdits(text, forward)

		if text != nextState.text {
			t.Errorf("redo %d text mismatch: got %q, want %q", i, text, nextState.text)
		}
		if !reflect.DeepEqual(grp.CursorsAfter, nextState.cur) {
			t.Errorf("redo %d cursor mismatch", i)
		}
	}

	// Check Redo truncation
	// Now we are at the end, undo a few times
	s, _, _ = s.Undo()
	s, _, _ = s.Undo()
	
	// Push a new edit
	s = s.Push(history.EditGroup{})
	if s.CanRedo() {
		t.Errorf("expected redo stack to be cleared after new push")
	}
}

func TestHistory_WhitespaceCoalescing(t *testing.T) {
	mockTime := time.Date(2024, 1, 1, 12, 0, 0, 0, time.UTC)
	s := history.New(func() time.Time { return mockTime })

	// Push a space
	s = s.Push(history.EditGroup{
		Edits: []buffer.AppliedEdit{{Start: 0, End: 0, Insert: " "}},
		Kind: history.EditInsertChar,
		Timestamp: mockTime,
	})

	// Try to coalesce a character immediately after
	mockTime = mockTime.Add(10 * time.Millisecond)
	if s.ShouldCoalesce(history.EditInsertChar, mockTime) {
		t.Errorf("expected ShouldCoalesce to be false after a whitespace, even within time window")
	}
}

func TestHistory_UndoRedo_Truncation(t *testing.T) {
	s := history.New(time.Now)

	s = s.Push(history.EditGroup{Kind: history.EditInsertChar}) // A
	s = s.Push(history.EditGroup{Kind: history.EditDeleteChar}) // B

	// Undo B
	var ok bool
	s, _, ok = s.Undo()
	if !ok {
		t.Fatalf("failed to undo")
	}

	if !s.CanRedo() {
		t.Fatalf("expected to be able to redo")
	}

	// Push C
	s = s.Push(history.EditGroup{Kind: history.EditBatch}) // C

	if s.CanRedo() {
		t.Fatalf("expected redo stack to be empty after new push")
	}

	// Make sure Undo goes to A
	s, grp, ok := s.Undo() // undos C
	if !ok || grp.Kind != history.EditBatch {
		t.Fatalf("expected to undo C, got %v", grp.Kind)
	}

	s, grp, ok = s.Undo() // undos A
	if !ok || grp.Kind != history.EditInsertChar {
		t.Fatalf("expected to undo A, got %v", grp.Kind)
	}

	if s.CanUndo() {
		t.Fatalf("expected no more undo")
	}
}
