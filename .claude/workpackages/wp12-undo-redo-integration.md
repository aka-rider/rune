# WP12 — Undo/Redo Integration

## Scope

Wire history package into editor component with full coalescing and cursor restoration

## Dependencies

- WP10 (basic editing — edits create history entries)
- WP11 (multi-cursor — multi-cursor edits form single group)

## Deliverables

### Commands

| Command | Key | Behavior |
|---------|-----|----------|
| `history.undo` | `Cmd+Z` | Revert most recent edit group. Restore cursors to pre-edit position. |
| `history.redo` | `Cmd+Shift+Z` | Re-apply most recently undone group. Restore cursors to post-edit position. |

### Coalescing Integration

**Ownership note:** WP8 delivers `applyOperation` in `apply.go` with a basic history.Push (no coalescing). WP12 **modifies** `apply.go` to add the coalescing logic below. WP12 also implements `applyUndo`/`applyRedo` (WP8 provides stubs that return no-op).

Wire `ShouldCoalesce` and `MergeIntoLast` into `applyOperation`:

```go
// In applyOperation:
if m.history.ShouldCoalesce(kind, now) {
    m.history = m.history.MergeIntoLast(appliedEdits, op.Cursors.All())
} else {
    m.history = m.history.Push(group)
}
```

**EditKind mapping from commands:**
- `edit.insert-character` → `EditInsertChar`
- `edit.delete-left`, `edit.delete-right` → `EditDeleteChar`
- `edit.newline` → `EditNewline`
- `clipboard.paste` → `EditPaste`
- `edit.move-line-up/down` → `EditMoveLine`
- `edit.clone-line-up/down` → `EditCloneLine`
- Multi-cursor batch → `EditBatch`

### Undo Flow (from spec §E)

1. `history.Undo()` → pops EditGroup
2. `InverseEdits()` → returns ascending order
3. Sort descending for ApplyEdits
4. `buf.ApplyEdits(inverseEdits)` → restored buffer
5. `cursors = group.CursorsBefore`
6. `dirty = (buf.Version() != savedVersion)`

### Redo Flow

1. `history.Redo()` → pops EditGroup
2. Reconstruct original edits from AppliedEdits (already descending)
3. `buf.ApplyEdits(originalEdits)`
4. `cursors = group.CursorsAfter`

### Tests (from qa-implementation-specs.md)

```go
// Coalescing
{"coalesce-fast-typing", ops("insert:a@0", "insert:b@100", "insert:c@200"), 1},  // 1 undo
{"break-on-idle", ops("insert:a@0", "insert:b@400"), 2},  // 2 undos
{"break-on-whitespace", ops("insert:a@0", "insert: @50"), 2},
{"break-on-delete", ops("insert:a@0", "delete-left@50"), 2},

// Cursor restoration
{"undo-insert", "hel|lo", "edit.insert-character", a("X"), "hel|lo"},  // after undo
{"undo-delete", "hel|lo", "edit.delete-left", nil, "hel|lo"},  // after undo
{"undo-multi", "a|b|c", "edit.insert-character", a("X"), "a|b|c"},  // after undo

// Redo cleared on new edit
// Undo, then new edit → CanRedo() == false
```

## Constraints

- Deterministic clock injection for coalescing tests
- No `time.Sleep` — use injected clock
- Multi-cursor edits = single undo group
- Redo truncation on any new edit after undo
- Under 500 LoC per file

## QA Gates

These are the definitive gates for user trust. If undo/redo is broken, the editor is unusable.

| # | Gate | Harm Prevented |
|---|------|----------------|
| 1 | **Trust test:** 20 varied operations (insert, delete, newline, move-line, indent, multi-cursor) + undo ALL → byte-identical to original content + cursor state | User cannot recover from mistakes — the safety net has holes |
| 2 | Undo across cursor-count change: 3 cursors → edit merges to 2 → undo → 3 cursors restored at original positions | Undo loses cursors → user had multi-cursor edit, undo only partially reverts |
| 3 | Multi-cursor edit = 1 undo group: 3 cursors insert "X" → single Cmd+Z reverts all 3 insertions | User must press undo N times for one logical operation → broken UX |
| 4 | Whitespace insertion breaks coalescing: type "ab " → 2 undo groups ("ab" then " ") | User types word + space, undoes → loses entire sentence instead of just the space |
| 5 | Redo invalidation: undo K groups, make ANY new edit → `CanRedo() == false` | User undoes, makes correction, accidentally presses redo → stale content spliced in |
| 6 | Undo of `move-line-up`: both content order AND cursor position restored | Line moves back but cursor stays in wrong position → user’s context lost |

**Testing approach:** Gate 1 via property loop (1000 random sequences with deterministic clock). Gates 2-6 via explicit table-driven scenarios.

## Verification

```bash
go test ./pkg/ui/components/editor/ -run TestSpec_Undo -v
go test ./pkg/ui/components/editor/ -run TestSpec_UndoCoalescing -v
go test ./pkg/ui/components/editor/ -run TestSpec_UndoRestoresCursors -v
go test ./pkg/ui/components/editor/ -run TestSpec_RedoClearedOnNewEdit -v
```
