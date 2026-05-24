# WP9 — Navigation Commands

## Scope

Navigation command implementations in `pkg/ui/components/editor/commands.go`

## Dependencies

- WP8 (editor component with command dispatch infrastructure)

## Deliverables

### 14 Navigation Commands

| Command | Key (macOS) | Behavior |
|---------|-------------|----------|
| `cursor.character-left` | `←` | Move one char left. At line start, wrap to end of previous line. |
| `cursor.character-right` | `→` | Move one char right. At line end, wrap to start of next line. |
| `cursor.line-up` | `↑` | Move one display row up. Preserve DesiredCol. |
| `cursor.line-down` | `↓` | Move one display row down. Preserve DesiredCol. |
| `cursor.word-left` | `Alt+←` | Move to start of current/previous word. |
| `cursor.word-right` | `Alt+→` | Move to end of current/next word. |
| `cursor.line-start` | `Cmd+←` | First non-whitespace; if there, column 0 (toggle). |
| `cursor.line-end` | `Cmd+→` | After last non-newline character. |
| `cursor.document-start` | `Cmd+↑` | Offset 0. |
| `cursor.document-end` | `Cmd+↓` | End of last line. |
| `cursor.page-up` | `Page Up` | Up by viewport height minus 2 lines overlap. |
| `cursor.page-down` | `Page Down` | Down by viewport height minus 2 lines overlap. |
| `scroll.line-up` | `Ctrl+↑` | Scroll viewport up, cursor clamps to visible. |
| `scroll.line-down` | `Ctrl+↓` | Scroll viewport down, cursor clamps to visible. |
| `scroll.character-left` | `Ctrl+←` (wrap OFF) | Scroll viewport left by one column. Only active when soft-wrap is OFF. |
| `scroll.character-right` | `Ctrl+→` (wrap OFF) | Scroll viewport right by one column. Only active when soft-wrap is OFF. |

### 11 Selection Commands (Shift composition)

Every navigation command has a `select.*` variant: moves Position but leaves Anchor fixed.

| Command | Key (macOS) |
|---------|-------------|
| `select.character-left` | `Shift+←` |
| `select.character-right` | `Shift+→` |
| `select.line-up` | `Shift+↑` |
| `select.line-down` | `Shift+↓` |
| `select.word-left` | `Shift+Alt+←` |
| `select.word-right` | `Shift+Alt+→` |
| `select.line-start` | `Shift+Cmd+←` |
| `select.line-end` | `Shift+Cmd+→` |
| `select.document-start` | `Shift+Cmd+↑` |
| `select.document-end` | `Shift+Cmd+↓` |
| `select.all` | `Cmd+A` |

### Selection collapse on navigation (without Shift)

- Forward selection `[text]`: navigation collapses to SelectionEnd
- Backward selection `]text[`: navigation collapses to SelectionStart
- Exception: left/right collapse to start/end respectively regardless of direction

### DesiredCol semantics

- Stored in Syntax Space columns (post-fold/expand, pre-wrap)
- Vertical movement preserves DesiredCol
- Horizontal movement resets DesiredCol to current column
- When cursor enters token that expands, DesiredCol recalculated

### Word boundary definition

Transition between `[a-zA-Z0-9_]` and non-word characters, or between whitespace and non-whitespace.

### Tests (table-driven using `internal/editortest` notation)

From `qa-implementation-specs.md`:

```go
// Navigation
{"left/mid", "hel|lo", "cursor.character-left", "he|llo"},
{"left/line-start-wraps", "hello\n|world", "cursor.character-left", "hello|\nworld"},
{"left/doc-start-noop", "|hello", "cursor.character-left", "|hello"},
{"left/collapses-fwd-sel", "he[ll]o", "cursor.character-left", "he|llo"},
{"right/line-end-wraps", "hello|\nworld", "cursor.character-right", "hello\n|world"},
{"word-left/mid-word", "hel|lo", "cursor.word-left", "|hello"},
{"line-start/toggle", "  |hello", "cursor.line-start", "|  hello"},

// Selection
{"sel-left/from-no-sel", "hel|lo", "select.character-left", "he]l[lo"},
{"sel-right/extend", "hel[l]o", "select.character-right", "hel[lo]"},
{"sel-all", "hel|lo", "select.all", "[hello]"},
```

Minimum 4 cases per command: happy path, boundary, selection interaction, multi-cursor.

## Constraints

- Commands return `command.Result` with `OperationKind = OperationMoveCursors`
- Commands operate on all cursors in CursorSet (multi-cursor fan-out)
- No buffer mutations in navigation commands
- Under 500 LoC per file (split commands across files if needed)

## QA Gates

These gates protect WP10 (editing depends on correct cursor positioning), WP11 (multi-cursor navigation), and user trust in cursor behavior.

| # | Gate | Harm Prevented |
|---|------|----------------|
| 1 | `character-left` at offset 0 = no-op; `character-right` at bufLen = no-op | Cursor escapes buffer bounds → panic on next edit |
| 2 | `line-start` toggle cycle: mid-line → first-nonwhitespace → col 0 → first-nonwhitespace (3-state cycle) | User pressing Home repeatedly doesn't reach column 0 or gets stuck |
| 3 | DesiredCol preserved: cursor at col 10, move down to short line (col 3), move down to long line → cursor back at col 10 | Vertical navigation destroys column position → user must re-position horizontally after every up/down |
| 4 | Selection collapse: forward selection `[text]` + `character-left` → cursor at anchor (left edge); `character-right` → cursor at position (right edge) | Wrong collapse direction → user's cursor jumps to unexpected side of selection |
| 5 | `word-left` on `café world` stops between `f` and `é` (ASCII-only word boundary per spec) | Word navigation inconsistent with spec definition → unpredictable jumps |
| 6 | All navigation commands with multi-cursor: each cursor moves independently, then merge if overlapping | Multi-cursor navigation produces wrong positions → subsequent edits go to wrong places |

**Testing approach:** Table-driven with editortest notation. Minimum 70 entries covering all 14 commands × boundary conditions.

## Verification

```bash
go test ./pkg/ui/components/editor/ -run TestSpec_Navigation -v
go test ./pkg/ui/components/editor/ -run TestSpec_SelectionNavigation -v
```
