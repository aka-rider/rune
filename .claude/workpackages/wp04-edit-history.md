# WP4 — Edit History (Undo/Redo)

## Scope

`pkg/editor/history/`

## Dependencies

- WP2 (buffer — `AppliedEdit`, `Edit` types)
- WP3 (cursor — `Cursor` type for snapshots)

## Deliverables

### `pkg/editor/history/history.go`

Full API from spec §C:

```go
package history

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

type UndoStack struct { /* internal: groups []EditGroup, index int */ }

func New() UndoStack
func (s UndoStack) Push(group EditGroup) UndoStack
func (s UndoStack) Undo() (UndoStack, EditGroup, bool)
func (s UndoStack) Redo() (UndoStack, EditGroup, bool)
func (s UndoStack) CanUndo() bool
func (s UndoStack) CanRedo() bool
func (s UndoStack) ShouldCoalesce(kind EditKind, now time.Time) bool
func (s UndoStack) MergeIntoLast(edits []buffer.AppliedEdit, cursorsAfter []cursor.Cursor) UndoStack

// InverseEdits on EditGroup — produces edits that undo the group
func (g EditGroup) InverseEdits() []buffer.Edit
```

### `pkg/editor/history/history_test.go`

**Layer 1 — Invariants:**
- N pushes + N undos = original content AND original cursors
- Undo + Redo = identity (content and cursors)
- New edit after undo clears redo stack
- InverseEdits applied to post-edit buffer = pre-edit buffer

**Layer 2 — Spec Scenarios:**
- Inverse correctness: 3-cursor example from spec (verify with concrete byte offsets)
- Coalescing rule 1: consecutive chars within 300ms merge
- Coalescing rule 2: whitespace forces new group
- Coalescing rule 3: deletion forces new group
- Coalescing rule 4: paste never coalesces
- Coalescing rule 5: newline never coalesces
- Coalescing rule 6: 300ms idle forces new group
- Redo truncation: push A, push B, undo, push C → redo stack empty
- MergeIntoLast updates CursorsAfter

## Key Algorithm: InverseEdits

From spec — walk forward edits ascending (lowest Start first), accumulate delta:

```
For each AppliedEdit{Start, End, Deleted, Insert}:
  adjustedStart = Start + cumulativeDeltaFromLowerEdits
  Inverse: Edit{Start: adjustedStart, End: adjustedStart + len(Insert), Insert: Deleted}
  cumulativeDelta += len(Insert) - (End - Start)
```

Returns inverse edits in ascending order. Caller reverses for ApplyEdits (descending).

## Constraints

- Clock injected as `func() time.Time` parameter (not interface)
- Value semantics (UndoStack returns new values)
- No `time.Sleep` in tests — use deterministic clock
- Under 500 LoC per file

## QA Gates

These gates protect WP12 (undo/redo integration) and are the foundation of the user's ability to recover from mistakes.

| # | Gate | Harm Prevented |
|---|------|----------------|
| 1 | **P3:** For ANY sequence of N Push operations, N Undo calls restore original content AND original cursor state (byte-identical) | User presses undo expecting recovery — gets wrong content or wrong cursor position |
| 2 | **P4:** For ANY Undo, subsequent Redo restores exact pre-undo state (byte-identical content + cursors) | Redo produces different state than what was undone — user confusion, lost work |
| 3 | After Undo then Push (new edit): `CanRedo() == false` | User undoes, makes new edit, presses redo — gets stale state spliced into current document |
| 4 | `InverseEdits()` applied to post-edit buffer produces pre-edit content | Wrong inverse = undo corrupts document instead of restoring it |
| 5 | Coalescing boundary: ops at 299ms gap coalesce (1 group), ops at 301ms gap split (2 groups) | Wrong boundary = user can't undo individual words (too much coalesced) or must press undo 50 times for one paragraph (too little) |
| 6 | Whitespace/delete/newline/paste each force new group regardless of timing | Undo granularity violates user expectation — can't undo just the space or just the paste |

**Testing approach:** P3/P4 via property loop ("trust test" — 1000 random operation sequences with deterministic clock). Gates 4-6 via table-driven with exact byte comparisons.

## Verification

```bash
go test ./pkg/editor/history/ -v
```
