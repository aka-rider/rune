# WP11 — Multi-Cursor Operations

## Scope

Multi-cursor commands + full algorithm from spec §F

## Dependencies

- WP10 (basic editing — multi-cursor builds on single-cursor editing)

## Deliverables

### 3 Multi-Cursor Commands

| Command | Key | Behavior |
|---------|-----|----------|
| `multicursor.add-above` | `Alt+Cmd+↑` | Add cursor one line above at same DesiredCol. Clamp to line end if shorter. |
| `multicursor.add-below` | `Alt+Cmd+↓` | Add cursor one line below at same DesiredCol. |
| `multicursor.escape` | routed `Cancel` (`Escape`) | Collapse to single (keep primary, discard secondaries). If no multi-cursor, collapse selection. If neither, propagate Cancel. Do not emit a separate physical `esc` resolver binding. |

### Full Multi-Cursor Edit Algorithm (spec §F)

All editing commands (WP10) must use this algorithm when multiple cursors exist:

**Phase 1 — Normalize:** Merge overlapping cursor selections. Sort ascending.

**Phase 2 — Generate Edits:** For each normalized cursor, produce `buffer.Edit`.

Carry the source cursor ID alongside each generated edit until final cursors are built. Sorting edits must not lose which cursor produced which final position.

**Phase 3 — Sort Descending:** Sort edits by Start descending (required by Buffer.ApplyEdits).

Use `buffer.CloneAndSortEditsDescending` or the shared editor edit builder from WP08/WP10. Do not hand-roll sort rules in each command.

**Phase 4 — Compute Post-Edit Positions:**
```go
// Walk ascending (lowest offset first), accumulate delta
for i := len(edits) - 1; i >= 0; i-- {
    positions[i] = edits[i].Start + len(edits[i].Insert) + cumulativeDelta
    delta := len(edits[i].Insert) - (edits[i].End - edits[i].Start)
    cumulativeDelta += delta
}
```

**Phase 5 — Apply:** `buf.ApplyEdits(sortedEditsDescending)`

**Phase 6 — Build CursorSet:** From computed positions, merge overlapping.

**Phase 7 — Record History:** All edits = ONE EditGroup.

### Line Operations with Multi-Cursor

- `edit.move-line-up/down` — map cursors to line ranges, unify overlapping ranges, operate on blocks
- `edit.clone-line-up/down` — same line-range unification
- `edit.delete-line` — delete each cursor's line (unified)

### Tests (from qa-implementation-specs.md)

```go
// Multi-cursor insert
{"multi-insert", "a|b|c|d", "edit.insert-character", a("X"), "aX|bX|cX|d"},

// Delete with adjacent cursors
{"multi-del-left/spaced", "a|bc|de|f", "edit.delete-left", nil, "|b|d|f"},

// Overlapping cursors merge
{"multi-del-left/merge", "a|b|c", "edit.delete-left", nil, "|c"},  // both cursors land together, then merge

// Add cursor below
{"add-below/basic", "hello|\nworld", "multicursor.add-below", nil, "hello|\nworld|"},
{"add-below/clamps", "hello world|\nhi", "multicursor.add-below", nil, "hello world|\nhi|"},

// Escape
{"escape/multi", "a|b|c", "multicursor.escape", nil, "a|bc"},
{"escape/sel-only", "h[ell]o", "multicursor.escape", nil, "hell|o"},

// Line operations with multi-cursor
{"move-up/basic", "aaa\nb|bb\nccc", "edit.move-line-up", nil, "b|bb\naaa\nccc"},
{"clone-down/basic", "a|aa\nbbb", "edit.clone-line-down", nil, "a|aa\naaa\nbbb"},
```

### Spec examples to validate:

**3 Cursors Insert 'x'** (from spec §F):
- Buffer: "abcdefghij", Cursors at 2, 5, 8
- After: "abxcdexfghxij", Cursors at 3, 7, 11

**Multi-Cursor with Selections** (from spec §F):
- Buffer: "Hello World Test", Cursors selecting "Hello" and "World"
- Replace with "X": "X X Test", Cursors at 1, 3

## Constraints

- Multi-cursor edit semantics: all edits apply independently, overlapping cursors merge after
- `multicursor.escape` is invoked from the routed `Cancel` path after modal overlay priority; `CommandBindings()` must not include physical `esc`.
- Edits apply in reverse-offset order (higher offsets first)
- Generated edits preserve cursor identity through sorting so final cursor positions map to the correct source cursor
- One EditGroup per multi-cursor command invocation
- Under 500 LoC per file

## QA Gates

These gates protect WP12 (undo must restore multi-cursor state), WP15 (clipboard paste distributes across cursors), and data integrity under compound operations.

| # | Gate | Harm Prevented |
|---|------|----------------|
| 1 | **P6:** 3 cursors insert "X" at offsets [1, 3, 5] in "abcdef" → result is "aXbXcXdef" with cursors at [2, 5, 8] | Reverse-offset algorithm bug → edits at wrong positions → garbled text |
| 2 | Adjacent cursors that collide after edit merge to 1 (not 2 overlapping cursors) | Overlapping cursors cause double-edit on same text region → data duplication or loss |
| 3 | `add-below` with cursor at end of long line + short next line → new cursor clamps to short line’s end | Cursor positioned past line end → out-of-bounds on next edit |
| 4 | `Escape` with multi-cursor = keep primary (lowest offset) only; single cursor + selection = collapse selection | Escape doesn’t clear multi-cursor → user thinks they have 1 cursor but has many → next edit affects multiple positions |
| 5 | Line-range unification: cursors on lines 0 and 1 + move-line-up = no-op (block already at top) | Partial move → some lines move, others don’t → document order corrupted |
| 6 | Multi-cursor edit produces single EditGroup (one undo reverts ALL cursors’ edits) | Multiple undo groups → user presses undo once, only some cursors revert → inconsistent state |
| 7 | Replacement selections with negative deltas compute final cursor positions correctly | Replacing longer selections with shorter text leaves cursors shifted to stale offsets |
| 8 | Cursor identity is preserved through descending edit sort | Primary/secondary cursors swap unpredictably after edits, breaking undo restoration |
| 9 | Focused editor receives real Escape key through routed Cancel and collapses multi-cursor/selection; `CommandBindings()` contains no physical `esc` binding | Routed cancel path is broken or duplicate Escape bindings reappear |

**Testing approach:** Concrete byte-offset scenarios with editortest notation. P6 via property test (1000 random multi-cursor configs + edits, verify each cursor's surrounding text).

## Verification

```bash
go test ./pkg/ui/components/editor/ -run TestSpec_MultiCursorEditing -v
```
