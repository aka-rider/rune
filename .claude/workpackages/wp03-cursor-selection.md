# WP3 — Cursor & Selection

## Scope

`pkg/editor/cursor/`

## Dependencies

- WP1 (coords types)
- WP2 (buffer — for `AppliedEdit` type in `AdjustAfterBatchEdits`)

## Deliverables

### `pkg/editor/cursor/cursor.go`

Full API from spec §B:

```go
package cursor

type Cursor struct {
    Position   int  // byte offset — the "head" (where cursor blinks)
    Anchor     int  // byte offset — the "tail". == Position when no selection
    DesiredCol int  // preserved column for vertical movement (Syntax Space)
    ID         int  // stable identifier
}

// Selection helpers
func (c Cursor) HasSelection() bool
func (c Cursor) SelectionStart() int
func (c Cursor) SelectionEnd() int
func (c Cursor) SelectionRange() (int, int)
func (c Cursor) Reversed() bool
func (c Cursor) CollapseToPosition() Cursor
func (c Cursor) CollapseToStart() Cursor
func (c Cursor) CollapseToEnd() Cursor

type CursorSet struct { /* internal: sorted []Cursor, nextID int */ }

func NewCursorSet(offset int) CursorSet
func NewCursorSetFrom(cursors []Cursor) CursorSet
func NewCursorSetFromPositions(positions []int) CursorSet  // for multi-cursor post-edit (spec §F Phase 6)
func (cs CursorSet) Primary() Cursor
func (cs CursorSet) All() []Cursor
func (cs CursorSet) Len() int
func (cs CursorSet) IsMulti() bool
func (cs CursorSet) Add(c Cursor) CursorSet
func (cs CursorSet) CollapseTo(primary Cursor) CursorSet
func (cs CursorSet) Merge() CursorSet
func (cs CursorSet) Map(fn func(Cursor) Cursor) CursorSet
func (cs CursorSet) MapWithIndex(fn func(int, Cursor) Cursor) CursorSet
func (cs CursorSet) AdjustAfterEdit(start, end int, insertLen int) CursorSet
func (cs CursorSet) AdjustAfterBatchEdits(edits []buffer.AppliedEdit) CursorSet
```

### `pkg/editor/cursor/cursor_test.go`

**Layer 1 — Invariants:**
- All cursors in [0, bufLen] after any operation
- After Merge(), cursors sorted ascending, no overlapping selections
- AdjustAfterEdit preserves relative ordering

**Layer 2 — Spec Scenarios:**
- Merge overlapping selections (spec algorithm: sort by SelectionStart, walk pairs)
- AdjustAfterEdit: offsets before Start unchanged, within [Start,End) collapse, after End shift
- AdjustAfterBatchEdits: 3-cursor example from spec §F
- Multi-cursor position computation (spec examples: 3 cursors insert 'x', selections replaced)
- CollapseToPosition/Start/End correctness
- Add + re-sort + ID assignment

## Constraints

- Value semantics throughout (CursorSet methods return new values)
- Sorted invariant maintained after every mutation
- Merge uses lower-ID-wins for stability
- Under 500 LoC per file

## QA Gates

These gates protect WP9-WP12 (all editing/navigation operates through CursorSet) and WP4 (history stores cursor snapshots).

| # | Gate | Harm Prevented |
|---|------|----------------|
| 1 | **P1:** No operation (Move, Extend, AdjustAfterEdit, AdjustAfterBatchEdits) produces Position or Anchor outside [0, bufLen] | Out-of-bounds cursor → panic on next buffer.Slice() or silent corruption on next edit |
| 2 | **P8:** After AdjustAfterBatchEdits with any valid edits, all cursor fields in [0, newBufLen] | Multi-cursor edit (WP11) shifts offsets — wrong adjustment = cursor pointing at garbage |
| 3 | After Merge(): `cursors[i].SelectionEnd() < cursors[i+1].SelectionStart()` for all i | Overlapping cursors cause double-edits on same region → data corruption |
| 4 | AdjustAfterEdit is monotonic: if cursor A.Position < B.Position before, then A.Position ≤ B.Position after | Cursor order inversion breaks the sorted invariant → merge algorithm produces wrong results |
| 5 | Merge preserves lower-ID cursor (stable identity) | Undo (WP4/WP12) restores cursors by matching state — unstable merge = wrong cursor restored |

**Testing approach:** P1/P8 via fuzz (random bufLen + random operations, assert bounds). Gates 3-5 via property loop (random cursor sets + random edits, assert postconditions).

## Verification

```bash
go test ./pkg/editor/cursor/ -v
```
