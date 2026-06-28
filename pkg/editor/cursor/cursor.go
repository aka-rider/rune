package cursor

import (
	"sort"

	"rune/pkg/editor/buffer"
)

type Cursor struct {
	Position   int // byte offset — the "head" (where cursor blinks)
	Anchor     int // byte offset — the "tail". == Position when no selection
	DesiredCol int // preserved column for vertical movement (Syntax Space)
	ID         int // stable identifier
}

func (c Cursor) HasSelection() bool {
	return c.Position != c.Anchor
}

func (c Cursor) SelectionStart() int {
	if c.Position < c.Anchor {
		return c.Position
	}
	return c.Anchor
}

func (c Cursor) SelectionEnd() int {
	if c.Position > c.Anchor {
		return c.Position
	}
	return c.Anchor
}

func (c Cursor) SelectionRange() (int, int) {
	if c.Position < c.Anchor {
		return c.Position, c.Anchor
	}
	return c.Anchor, c.Position
}

func (c Cursor) Reversed() bool {
	return c.Position < c.Anchor
}

func (c Cursor) CollapseToPosition() Cursor {
	return Cursor{
		Position:   c.Position,
		Anchor:     c.Position,
		DesiredCol: c.DesiredCol,
		ID:         c.ID,
	}
}

func (c Cursor) CollapseToStart() Cursor {
	start := c.SelectionStart()
	return Cursor{
		Position:   start,
		Anchor:     start,
		DesiredCol: c.DesiredCol,
		ID:         c.ID,
	}
}

func (c Cursor) CollapseToEnd() Cursor {
	end := c.SelectionEnd()
	return Cursor{
		Position:   end,
		Anchor:     end,
		DesiredCol: c.DesiredCol,
		ID:         c.ID,
	}
}

type CursorSet struct {
	cursors []Cursor
	nextID  int
}

func NewCursorSet(offset int) CursorSet {
	return CursorSet{
		cursors: []Cursor{{Position: offset, Anchor: offset, ID: 1}},
		nextID:  2,
	}
}

func NewCursorSetFrom(cursors []Cursor) CursorSet {
	if len(cursors) == 0 {
		return NewCursorSet(0)
	}

	cp := make([]Cursor, len(cursors))
	copy(cp, cursors)

	maxID := 0
	for i := range cp {
		if cp[i].ID > maxID {
			maxID = cp[i].ID
		}
	}

	for i := range cp {
		if cp[i].ID == 0 {
			maxID++
			cp[i].ID = maxID
		}
	}

	cs := CursorSet{
		cursors: cp,
		nextID:  maxID + 1,
	}
	return cs.Merge()
}

func NewCursorSetFromPositions(positions []int) CursorSet {
	if len(positions) == 0 {
		return NewCursorSet(0)
	}
	cursors := make([]Cursor, len(positions))
	for i, p := range positions {
		cursors[i] = Cursor{Position: p, Anchor: p, ID: i + 1}
	}
	cs := CursorSet{
		cursors: cursors,
		nextID:  len(positions) + 1,
	}
	return cs.Merge()
}

func (cs CursorSet) Primary() Cursor {
	if len(cs.cursors) == 0 {
		return Cursor{}
	}
	return cs.cursors[0]
}

func (cs CursorSet) All() []Cursor {
	cp := make([]Cursor, len(cs.cursors))
	copy(cp, cs.cursors)
	return cp
}

func (cs CursorSet) Len() int {
	return len(cs.cursors)
}

func (cs CursorSet) IsMulti() bool {
	return len(cs.cursors) > 1
}

func (cs CursorSet) Add(c Cursor) CursorSet {
	if c.ID == 0 {
		c.ID = cs.nextID
	}
	cs.nextID++

	cp := make([]Cursor, len(cs.cursors), len(cs.cursors)+1)
	copy(cp, cs.cursors)
	cp = append(cp, c)

	res := CursorSet{
		cursors: cp,
		nextID:  cs.nextID,
	}
	return res.Merge()
}

func (cs CursorSet) CollapseTo(primary Cursor) CursorSet {
	return CursorSet{
		cursors: []Cursor{primary},
		nextID:  cs.nextID,
	}
}

func (cs CursorSet) Merge() CursorSet {
	if len(cs.cursors) <= 1 {
		return cs
	}

	cp := make([]Cursor, len(cs.cursors))
	copy(cp, cs.cursors)

	sort.SliceStable(cp, func(i, j int) bool {
		startI := cp[i].SelectionStart()
		startJ := cp[j].SelectionStart()
		if startI != startJ {
			return startI < startJ
		}
		endI := cp[i].SelectionEnd()
		endJ := cp[j].SelectionEnd()
		if endI != endJ {
			return endI < endJ
		}
		return cp[i].ID < cp[j].ID
	})

	merged := make([]Cursor, 0, len(cp))
	current := cp[0]

	for i := 1; i < len(cp); i++ {
		next := cp[i]

		if current.SelectionEnd() >= next.SelectionStart() {
			survivorID := current.ID
			if next.ID < survivorID {
				survivorID = next.ID
			}

			start := current.SelectionStart()
			end := current.SelectionEnd()
			if next.SelectionEnd() > end {
				end = next.SelectionEnd()
			}

			isReversed := false
			if current.ID == survivorID {
				isReversed = current.Reversed()
			} else {
				isReversed = next.Reversed()
			}

			var pos, anc int
			if isReversed {
				pos = start
				anc = end
			} else {
				pos = end
				anc = start
			}

			current = Cursor{
				Position:   pos,
				Anchor:     anc,
				DesiredCol: current.DesiredCol,
				ID:         survivorID,
			}
		} else {
			merged = append(merged, current)
			current = next
		}
	}
	merged = append(merged, current)

	return CursorSet{
		cursors: merged,
		nextID:  cs.nextID,
	}
}

func (cs CursorSet) Map(fn func(Cursor) Cursor) CursorSet {
	cp := make([]Cursor, len(cs.cursors))
	for i, c := range cs.cursors {
		cp[i] = fn(c)
	}
	res := CursorSet{
		cursors: cp,
		nextID:  cs.nextID,
	}
	return res.Merge()
}

func (cs CursorSet) MapWithIndex(fn func(int, Cursor) Cursor) CursorSet {
	cp := make([]Cursor, len(cs.cursors))
	for i, c := range cs.cursors {
		cp[i] = fn(i, c)
	}
	res := CursorSet{
		cursors: cp,
		nextID:  cs.nextID,
	}
	return res.Merge()
}

func (cs CursorSet) AdjustAfterEdit(start, end int, insertLen int) CursorSet {
	return cs.Map(func(c Cursor) Cursor {
		adjust := func(pos int) int {
			if pos < start {
				return pos
			}
			if pos < end {
				return start + insertLen
			}
			return pos + (insertLen - (end - start))
		}
		c.Position = adjust(c.Position)
		c.Anchor = adjust(c.Anchor)
		return c
	})
}

func (cs CursorSet) AdjustAfterBatchEdits(edits []buffer.AppliedEdit) CursorSet {
	return cs.Map(func(c Cursor) Cursor {
		adjust := func(pos int) int {
			shift := 0
			// edits are descending, so len-1 is leftmost
			for i := len(edits) - 1; i >= 0; i-- {
				ae := edits[i]
				oldStart := ae.Start - shift
				oldEnd := oldStart + len(ae.Deleted)

				if pos < oldStart {
					return pos + shift
				}
				if pos < oldEnd {
					return ae.Start + len(ae.Insert)
				}

				shift += len(ae.Insert) - len(ae.Deleted)
			}
			return pos + shift
		}

		c.Position = adjust(c.Position)
		c.Anchor = adjust(c.Anchor)
		return c
	})
}
