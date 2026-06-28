package cursor

import (
	"testing"

	"rune/pkg/editor/buffer"
)

func TestCursorSelectionHelpers(t *testing.T) {
	c := Cursor{Position: 10, Anchor: 5}
	if !c.HasSelection() {
		t.Error("Expected HasSelection() to be true")
	}
	if c.SelectionStart() != 5 {
		t.Errorf("Expected SelectionStart to be 5, got %d", c.SelectionStart())
	}
	if c.SelectionEnd() != 10 {
		t.Errorf("Expected SelectionEnd to be 10, got %d", c.SelectionEnd())
	}
	s, e := c.SelectionRange()
	if s != 5 || e != 10 {
		t.Errorf("Expected SelectionRange to be (5, 10), got (%d, %d)", s, e)
	}
	if c.Reversed() {
		t.Error("Expected Reversed() to be false")
	}

	c2 := Cursor{Position: 5, Anchor: 10}
	if !c2.Reversed() {
		t.Error("Expected Reversed() to be true")
	}
}

func TestCursorCollapse(t *testing.T) {
	c := Cursor{Position: 10, Anchor: 5, DesiredCol: 1, ID: 42}

	cPos := c.CollapseToPosition()
	if cPos.Position != 10 || cPos.Anchor != 10 || cPos.DesiredCol != 1 || cPos.ID != 42 {
		t.Errorf("CollapseToPosition failed: %+v", cPos)
	}

	cStart := c.CollapseToStart()
	if cStart.Position != 5 || cStart.Anchor != 5 || cStart.DesiredCol != 1 || cStart.ID != 42 {
		t.Errorf("CollapseToStart failed: %+v", cStart)
	}

	cEnd := c.CollapseToEnd()
	if cEnd.Position != 10 || cEnd.Anchor != 10 || cEnd.DesiredCol != 1 || cEnd.ID != 42 {
		t.Errorf("CollapseToEnd failed: %+v", cEnd)
	}
}

func TestCursorSetMergeOverlapping(t *testing.T) {
	// Overlapping selections
	cs := NewCursorSetFrom([]Cursor{
		{Position: 5, Anchor: 0, ID: 1},
		{Position: 10, Anchor: 3, ID: 2},
	})

	if cs.Len() != 1 {
		t.Fatalf("Expected 1 merged cursor, got %d", cs.Len())
	}
	c := cs.Primary()
	// Should merge to (0, 10), keeping ID 1 (lower wins).
	// ID 1 is not reversed (pos=5, anc=0). So pos=10, anc=0.
	if c.Position != 10 || c.Anchor != 0 || c.ID != 1 {
		t.Errorf("Merge failed: %+v", c)
	}
}

func TestCursorSetAdjustAfterEdit(t *testing.T) {
	cs := NewCursorSetFrom([]Cursor{
		{Position: 5, Anchor: 5, ID: 1},   // before
		{Position: 15, Anchor: 10, ID: 2}, // overlaps
		{Position: 20, Anchor: 20, ID: 3}, // after
	})

	// Edit: delete [12, 16), replace with 2 chars
	// start=12, end=16, insertLen=2. diff = -2
	adj := cs.AdjustAfterEdit(12, 16, 2)

	cursors := adj.All()
	if cursors[0].Position != 5 {
		t.Errorf("C1 position should be untouched: %d", cursors[0].Position)
	}
	if cursors[1].Anchor != 10 {
		t.Errorf("C2 anchor should be untouched: %d", cursors[1].Anchor)
	}
	if cursors[1].Position != 14 { // was 15, which is inside [12, 16), so collapsed to 12+2=14
		t.Errorf("C2 position should be collapsed to start+insertLen: %d", cursors[1].Position)
	}
	if cursors[2].Position != 18 { // was 20, after edit, shifted by -2
		t.Errorf("C3 position should be shifted: %d", cursors[2].Position)
	}
}

func TestCursorSetAdjustAfterBatchEdits(t *testing.T) {
	cs := NewCursorSetFrom([]Cursor{
		{Position: 5, Anchor: 5, ID: 1},
		{Position: 10, Anchor: 10, ID: 2},
		{Position: 15, Anchor: 15, ID: 3},
	})

	buf := buffer.New("01234567890123456789")
	_, edits, _ := buf.ApplyEdits([]buffer.Edit{
		{Start: 15, End: 15, Insert: "w"},
		{Start: 10, End: 10, Insert: "x"},
		{Start: 5, End: 5, Insert: "y"},
	})

	// C1 was 5, now after start=5 (+1) -> 6
	// C2 was 10, now after start=5 (+1) and start=10 (+1) -> 12
	// C3 was 15, now after all 3 (+3) -> 18

	adj := cs.AdjustAfterBatchEdits(edits)
	cursors := adj.All()

	if cursors[0].Position != 6 {
		t.Errorf("C1 expected 6, got %d", cursors[0].Position)
	}
	if cursors[1].Position != 12 {
		t.Errorf("C2 expected 12, got %d", cursors[1].Position)
	}
	if cursors[2].Position != 18 {
		t.Errorf("C3 expected 18, got %d", cursors[2].Position)
	}
}

func TestCursorSetGates(t *testing.T) {
	// Gate 3: No overlapping after Merge
	cs := NewCursorSetFrom([]Cursor{
		{Position: 2, Anchor: 5, ID: 1},
		{Position: 4, Anchor: 7, ID: 2},
		{Position: 10, Anchor: 10, ID: 3},
		{Position: 6, Anchor: 8, ID: 4}, // overlaps with former
	})

	all := cs.All()
	if len(all) != 2 {
		t.Fatalf("Expected 2 cursors, got %d", len(all))
	}
	for i := 0; i < len(all)-1; i++ {
		if all[i].SelectionEnd() >= all[i+1].SelectionStart() {
			t.Errorf("Overlapping cursors: %+v and %+v", all[i], all[i+1])
		}
	}

	// Gate 4: Monotonic AdjustAfterEdit
	c1, c2 := cs.All()[0], cs.All()[1]
	if c1.Position > c2.Position {
		t.Errorf("Before adjust not monotonic")
	}
	adj := cs.AdjustAfterEdit(5, 5, 0)
	ac1, ac2 := adj.All()[0], adj.All()[1]
	if ac1.Position > ac2.Position {
		t.Errorf("After adjust not monotonic")
	}

	// Gate 5: Merge maintains lower ID
	if ac1.ID != 1 {
		t.Errorf("Lower ID not maintained, got %d", ac1.ID)
	}
}

func TestCursorSetAdd(t *testing.T) {
	cs := NewCursorSetFromPositions([]int{10, 20})
	if cs.Len() != 2 {
		t.Fatalf("Expected 2, got %d", cs.Len())
	}
	if cs.nextID != 3 {
		t.Errorf("Expected nextID 3, got %d", cs.nextID)
	}

	cs2 := cs.Add(Cursor{Position: 15, Anchor: 15})
	if cs2.Len() != 3 {
		t.Fatalf("Expected 3, got %d", cs2.Len())
	}
	all := cs2.All()
	if all[1].Position != 15 {
		t.Errorf("Expected 15 at index 1 due to sorting, got %d", all[1].Position)
	}
	if all[1].ID != 3 {
		t.Errorf("Expected assigned ID 3, got %d", all[1].ID)
	}
}
