# WP18 — Find/Replace

## Scope

MVP find/replace stubs only. `editor-spec.md` marks find/replace as Phase 2. This workpackage registers commands, adds overlay state skeleton, verifies Escape priority, and returns a surfaced disabled/not-implemented result without mutating the buffer.

Full find/replace implementation belongs in a future package (for example `wp20-find-replace-full.md`) unless Phase 2 is explicitly approved.

## Dependencies

- WP8 (editor component command dispatch and overlay ownership)
- WP11 (Escape priority must preserve multi-cursor state when overlay is open)

## Deliverables

### 6 Find/Replace Commands (registered stubs)

| Command | Key | Behavior |
|---------|-----|----------|
| `find.open` | `Cmd+F` | Open find overlay at top of editor. Focus search input. |
| `find.replace-open` | `Cmd+H` | Open find+replace overlay. |
| `find.next` | `Cmd+G` | Stub: surface disabled/not-implemented feedback; no buffer/cursor/history mutation. Overlay-local Enter may invoke this stub directly but is not a central key binding. |
| `find.previous` | `Cmd+Shift+G` | Stub: surface disabled/not-implemented feedback; no buffer/cursor/history mutation. Overlay-local Shift+Enter may invoke this stub directly but is not a central key binding. |
| `find.replace` | overlay-local command only | Stub: surface disabled/not-implemented feedback; no buffer/cursor/history mutation. |
| `find.replace-all` | overlay-local command only | Stub: surface disabled/not-implemented feedback; no buffer/cursor/history mutation. |

### Find Overlay State Skeleton

A child component within the editor (not a separate top-level component):

```go
type FindOverlay struct {
    visible     bool
    replaceMode bool
    query       string
    replacement string
    matches     []MatchRange  // byte offset ranges
    currentIdx  int           // which match is active
    caseSensitive bool
    useRegex    bool
}

type MatchRange struct {
    Start int
    End   int
}
```

### Keyboard Navigation in Overlay (MVP)

When the find overlay is visible, it owns every keypress before the editor buffer resolver. Only explicit overlay actions run. Printable characters, Backspace/Delete, arrows, Tab, Enter, Shift+Enter, Escape, undo/redo, paste/cut/copy, select-all, word-delete, navigation, and editing shortcuts are consumed by overlay logic or disabled MVP commands. None of these keypresses may dispatch editor buffer/history/clipboard commands while the overlay is visible.

Overlay-local keys (`Enter`, `Shift+Enter`, `Tab`, `Escape`) are not added as separate physical command bindings in `pkg/ui/keymap`. They are handled inside the overlay branch after the central `find.open` / `find.replace-open` command has made the overlay visible.

When visible, find overlay makes `editor.Model.WantsModalInput()` return true so workspace globals do not preempt overlay input.

- `Escape`: close overlay, return focus to editor. **Priority rule:** When find overlay is visible, Escape closes it (takes priority over `multicursor.escape`). When overlay is not visible, Escape falls through to multi-cursor collapse (WP11).
- `Enter`: invokes disabled `find.next` stub in MVP.
- `Shift+Enter`: invokes disabled `find.previous` stub in MVP.
- `Tab`: overlay-visible Tab is always consumed by the overlay. In replace mode it switches between find and replace inputs; in find-only mode it no-ops (or opens replace mode only if explicitly designed later). It must not fall through to editor indentation.

### Future Package Only

The following sections describe the future full implementation. Do not implement them in this MVP package and do not add tests expecting this behavior here:

### Match Highlighting

- All matches highlighted in display (distinct style from selection)
- Current match highlighted differently (brighter/outlined)
- Match count shown in overlay: "3 of 17"

### Search Behavior

- Incremental search: update matches as query changes
- Wrap around: after last match, next goes to first
- Case sensitivity toggle
- Regex toggle (compile regex, handle invalid patterns gracefully)

### Replace Behavior

- `find.replace`: replace current match text with replacement, advance cursor to next match
- `find.replace-all`: batch replace all matches (one undo group)
- Both use standard edit infrastructure (`applyOperation` with `EditBatch` kind)

### Tests

```go
{"find/open-stub", "hello|", "find.open", overlay visible and focused, content unchanged},
{"find/replace-open-stub", "hello|", "find.replace-open", replaceMode true, content unchanged},
{"find/next-disabled", "hello|", "find.next", surfaced disabled result, content unchanged},
{"find/printable-consumed", overlay open, type "abc", overlay query changes or consumes input, buffer unchanged},
{"find/backspace-consumed", overlay open, Backspace changes overlay input or no-ops, buffer unchanged},
{"find/delete-consumed", overlay open, Delete changes overlay input or no-ops, buffer unchanged},
{"find/arrows-consumed", overlay open, arrow keys move overlay cursor or no-op, buffer cursor unchanged},
{"find/enter-disabled", overlay open, Enter invokes disabled next stub, buffer unchanged},
{"find/shift-enter-disabled", overlay open, Shift+Enter invokes disabled previous stub, buffer unchanged},
{"find/cmd-g-disabled", overlay open, Cmd+G disabled, buffer unchanged},
{"find/cmd-shift-g-disabled", overlay open, Cmd+Shift+G disabled, buffer unchanged},
{"find/replace-disabled", replace overlay open, find.replace disabled, buffer/history unchanged},
{"find/replace-all-disabled", replace overlay open, find.replace-all disabled, buffer/history unchanged},
{"find/undo-redo-consumed", overlay open, Cmd+Z/Cmd+Shift+Z consumed, buffer/history unchanged},
{"find/clipboard-consumed", overlay open, Cmd+V/Cmd+X/Cmd+C consumed or overlay-local, buffer unchanged},
{"find/select-all-consumed", overlay open, Cmd+A selects overlay input or no-ops, editor selection unchanged},
{"find/word-delete-consumed", overlay open, Alt+Backspace/Alt+Delete consumed, buffer unchanged},
{"find/tab-consumed", overlay open in find-only mode, Tab consumed, content/cursors/history unchanged},
{"find/escape-priority", overlay open with multi-cursor, Escape closes overlay and preserves cursors},
```

## Constraints

- Find overlay is internal to editor component (not a separate component at page level)
- MVP stubs must not mutate buffer, cursor set, history, or clipboard
- Full replace-all single undo group is deferred to the future full package
- Under 500 LoC per file

## QA Gates

These gates ensure find/replace doesn't corrupt documents and integrates correctly with undo.

| # | Gate | Harm Prevented |
|---|------|----------------|
| 1 | `find.open` and `find.replace-open` only show overlay state and do not mutate content | Stub command accidentally changes document while feature is incomplete |
| 2 | Disabled `find.next/previous/replace/replace-all` surface clear not-implemented feedback | User invokes command and nothing happens silently |
| 3 | Escape priority: find overlay open → Escape closes overlay (does NOT collapse multi-cursor or propagate) | User presses Escape to close find, but multi-cursor collapses too → lost cursor positions |
| 4 | Full find/replace behavior is documented as deferred, not half-implemented | Worker ships incomplete replace logic under a passing stub package |
| 5 | Overlay-visible Delete, arrows, Enter, Shift+Enter, Cmd+G, Cmd+Shift+G, replace, and replace-all do not mutate buffer/cursors/history | Modal overlay leaks keys into the editor underneath |
| 6 | Overlay-visible undo/redo, clipboard, select-all, word-delete, navigation, and editing shortcuts are consumed before editor dispatch | Modal overlay allows document-changing shortcuts through |

**Testing approach:** Table-driven stub tests with content before/after equality. No match-count, replacement, or undo-integration tests belong in this MVP package.

## Verification

```bash
go test ./pkg/ui/components/editor/ -run TestFindStub -v
```
