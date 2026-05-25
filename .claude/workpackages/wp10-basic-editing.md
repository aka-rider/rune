# WP10 — Basic Editing Commands

## Scope

Editing command implementations in editor component

## Dependencies

- WP8 (editor component)
- WP9 (navigation — editing builds on cursor positioning)

## Deliverables

### 14 Editing Commands

| Command | Key | Behavior |
|---------|-----|----------|
| `edit.insert-character` | printable key | Insert char at cursor. Selection → replace first. Multi-cursor → insert at each. |
| `edit.newline` | routed `PrimaryAction` (`Enter`) | Insert `\n`. Auto-indent: copy leading whitespace from current line. Selection → replace. Do not emit a separate physical `enter` resolver binding. |
| `edit.delete-left` | `Backspace` | Selection → delete selected. Else delete one char left. Line start → join with previous. |
| `edit.delete-right` | `Delete` | Selection → delete selected. Else delete one char right. Line end → join with next. |
| `edit.delete-word-left` | `Alt+Backspace` | Selection → delete. Else delete to start of current/previous word. |
| `edit.delete-word-right` | `Alt+Delete` | Selection → delete. Else delete to end of current/next word. |
| `edit.delete-line` | `Cmd+Shift+K` | Delete entire current line including newline. Cursor → same column on next line. |
| `edit.move-line-up` | `Alt+↑` | Swap current line with line above. Selection spans → move block. At top: no-op. |
| `edit.move-line-down` | `Alt+↓` | Swap current line with line below. Selection spans → move block. At bottom: no-op. |
| `edit.clone-line-up` | `Alt+Shift+↑` | Duplicate line/block above. Cursor remains on original (moves down). |
| `edit.clone-line-down` | `Alt+Shift+↓` | Duplicate line/block below. Cursor remains on original. |
| `edit.indent` | `Tab` | Multi-line selection → indent all lines. Else insert tab/spaces at cursor. |
| `edit.outdent` | `Shift+Tab` | Remove one indentation level from current/selected lines. |
| `edit.toggle-comment` | `Cmd+/` | Toggle line comment. Multi-line → comment all. |

### Implementation details

All editing commands must use shared editor helpers from WP08/WP11 for normalization, descending sort, post-edit cursor positions, and history handoff. Do not implement a private multi-cursor edit algorithm inside individual command functions.

**insert-character:**
- For each cursor: if HasSelection → Edit{Start: SelectionStart, End: SelectionEnd, Insert: char}
- Else → Edit{Start: Position, End: Position, Insert: char}
- Sort edits descending, compute post-edit positions per §F algorithm

**newline with auto-indent:**
- Extract leading whitespace from current line: `regexp: ^[ \t]*`
- Insert `\n` + captured whitespace
- Post-edit cursor at end of inserted whitespace

**delete-left at line start:**
- Join: Edit{Start: LineStart(curLine)-1, End: LineStart(curLine), Insert: ""}
- Removes the `\n` at end of previous line

**delete-line:**
- If not last line: Edit{Start: LineStart(n), End: LineStart(n+1), Insert: ""}
- If last line and not first: Edit{Start: LineEnd(n-1), End: LineEnd(n), Insert: ""}
- If only line: Edit{Start: 0, End: Len(), Insert: ""}

**indent/outdent:**
- Indent: prepend tab/spaces to each line in selection range
- Outdent: remove leading tab or up to N spaces from each line
- Default `IndentConfig`: `UseTabs=true`, `TabSize=4`. If later configuration changes this default, update tests and help text in the same change.

**toggle-comment:**
- Phase-safe behavior before WP14: use markdown HTML comments only (`<!-- line -->` / remove) and do not try to infer code-fence language.
- After WP14 adds syntax context, use a `SyntaxContextAt(offset)`-style query to detect code fences and language-appropriate prefixes (`//`, `#`, etc.). Do not make WP10 import or parse markdown directly.

### Tests (table-driven from qa-implementation-specs.md)

```go
{"insert/normal", "hel|lo", "edit.insert-character", a("X"), "helX|lo"},
{"insert/replaces-sel", "h[ell]o", "edit.insert-character", a("X"), "hX|o"},
{"insert/multi-cursor", "a|b|c", "edit.insert-character", a("X"), "aX|bX|c"},
{"newline/basic", "hello|", "edit.newline", nil, "hello\n|"},
{"newline/auto-indent", "  hello|", "edit.newline", nil, "  hello\n  |"},
{"del-left/mid", "hel|lo", "edit.delete-left", nil, "he|lo"},
{"del-left/line-start-joins", "hello\n|world", "edit.delete-left", nil, "hello|world"},
{"del-left/doc-start-noop", "|hello", "edit.delete-left", nil, "|hello"},
{"del-right/line-end-joins", "hello|\nworld", "edit.delete-right", nil, "hello|world"},
{"del-line/mid", "aaa\nb|bb\nccc", "edit.delete-line", nil, "aaa\n|ccc"},
{"move-up/basic", "aaa\nb|bb\nccc", "edit.move-line-up", nil, "b|bb\naaa\nccc"},
{"move-up/at-top-noop", "a|aa\nbbb", "edit.move-line-up", nil, "a|aa\nbbb"},
{"move-down/basic", "a|aa\nbbb\nccc", "edit.move-line-down", nil, "bbb\na|aa\nccc"},
{"clone-down/basic", "a|aa\nbbb", "edit.clone-line-down", nil, "a|aa\naaa\nbbb"},
{"clone-up/basic", "a|aa\nbbb", "edit.clone-line-up", nil, "aaa\na|aa\nbbb"},
{"indent/no-sel", "hel|lo", "edit.indent", nil, "\thel|lo"},
{"outdent/indented", "\t|hello", "edit.outdent", nil, "|hello"},
```

## Constraints

- All editing commands return `OperationKind = OperationEditBuffer`
- `edit.newline` is invoked from the routed `PrimaryAction` path when editor is focused and no modal overlay owns Enter; `CommandBindings()` must not include physical `enter`.
- Edits sorted descending in Operation.Edits
- Commands handle selection-active case first (replace selection)
- Multi-cursor: fan out to all cursors, merge overlapping after
- Each edit operation properly records history kind for coalescing
- Invalid edit generation leaves pre-state intact and surfaces a hard error; never apply a partial batch.
- Under 500 LoC per file

## QA Gates

These gates protect WP11 (multi-cursor editing builds on single-cursor), WP12 (undo must correctly invert these edits), and data integrity.

| # | Gate | Harm Prevented |
|---|------|----------------|
| 1 | `delete-left` at line-start joins with previous line: `"hello\n\|world"` → `"hello\|world"` (byte-exact) | Line-join produces wrong content → user loses the newline but text gets mangled |
| 2 | `newline` auto-indent copies whitespace from current line: `"  hello\|"` → `"  hello\n  \|"` | Wrong indent source → new line gets indent from wrong line (stale or global state) |
| 3 | `move-line-up` at line 0 = no-op (content byte-identical to input) | First line moves “up” into oblivion → data loss |
| 4 | `delete-line` on single-line buffer (only content) → empty buffer, cursor at 0 | Edge case produces negative offset or leaves orphan newline |
| 5 | `indent` with multi-line selection indents ALL selected lines (not just cursor line) | Only cursor line indented → user's multi-line reformatting is incomplete |
| 6 | All editing commands with active selection: selection content is replaced (not preserved alongside insert) | Selection not cleared → insert duplicates text instead of replacing |
| 7 | `clone-line-down` on buffer without trailing newline: `"hello\|"` → `"hello\|\nhello"` | Missing separator → lines concatenated into garbled single line |
| 8 | `toggle-comment` before WP14 uses markdown comments and does not inspect code-fence language | Hidden dependency on future parser blocks WP10 or leads to ad hoc parsing |
| 9 | Focused editor receives real Enter key through `Update` and inserts newline; `CommandBindings()` contains no physical `enter` binding | Routed key path is broken even though command-name tests pass |

**Testing approach:** Table-driven with editortest notation. Minimum 60 entries. Each gate is a specific table entry that must pass.

## Verification

```bash
go test ./pkg/ui/components/editor/ -run TestSpec_Editing -v
```
