# WP18 — Find/Replace

## Scope

Find/replace overlay and commands (spec marks as "Phase 2 — stubbed" but includes full behavior)

## Dependencies

- WP9 (navigation — find.next/previous uses cursor movement)
- WP10 (editing — replace uses edit infrastructure)

## Deliverables

### 6 Find/Replace Commands

| Command | Key | Behavior |
|---------|-----|----------|
| `find.open` | `Cmd+F` | Open find overlay at top of editor. Focus search input. |
| `find.replace-open` | `Cmd+H` | Open find+replace overlay. |
| `find.next` | `Cmd+G` or `Enter` in find input | Jump to next match. |
| `find.previous` | `Cmd+Shift+G` or `Shift+Enter` | Jump to previous match. |
| `find.replace` | (in replace input) | Replace current match and advance to next. |
| `find.replace-all` | (in replace input) | Replace all matches in document. |

### Find Overlay Component

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

### Keyboard Navigation in Overlay

- `Escape`: close overlay, return focus to editor. **Priority rule:** When find overlay is visible, Escape closes it (takes priority over `multicursor.escape`). When overlay is not visible, Escape falls through to multi-cursor collapse (WP11).
- `Enter`: find next (in find input)
- `Shift+Enter`: find previous
- `Tab`: switch between find and replace inputs (when replace mode)

### Tests

```go
// Basic find
{"find/basic", "hello world hello", query: "hello", matchCount: 2},
{"find/case-insensitive", "Hello hello", query: "hello", caseSensitive: false, matchCount: 2},
{"find/regex", "foo123bar456", query: `\d+`, useRegex: true, matchCount: 2},

// Navigation
{"find-next/wraps", at last match, find.next → goes to first match},
{"find-prev/wraps", at first match, find.previous → goes to last match},

// Replace
{"replace/single", "aaa bbb aaa", replace "aaa" with "X", result: "X bbb aaa", cursor at next match},
{"replace-all", "aaa bbb aaa", replace "aaa" with "X", result: "X bbb X"},

// Undo
{"replace-all-undo", after replace-all, undo → original content restored},
```

## Constraints

- Find overlay is internal to editor component (not a separate component at page level)
- Replace-all creates single undo group
- Match computation does not block Update (for large files, consider async or chunked)
- Under 500 LoC per file

## QA Gates

These gates ensure find/replace doesn't corrupt documents and integrates correctly with undo.

| # | Gate | Harm Prevented |
|---|------|----------------|
| 1 | Replace-all produces single undo group (one Cmd+Z reverts ALL replacements) | User replace-alls 50 occurrences, realizes mistake, must press undo 50 times |
| 2 | Search wraps around: at last match, `find.next` returns to first match | User thinks there are no more matches, misses occurrences earlier in document |
| 3 | Escape priority: find overlay open → Escape closes overlay (does NOT collapse multi-cursor or propagate) | User presses Escape to close find, but multi-cursor collapses too → lost cursor positions |
| 4 | Replace preserves cursor positioning: after replace, cursor advances to next match | Cursor stays on replaced text → user presses replace again and replaces the replacement |
| 5 | Invalid regex pattern shows error in overlay, does not panic or corrupt state | Bad regex crashes editor or produces empty match set silently |

**Testing approach:** Table-driven with content + query + expected matches/replacements. Undo integration via trust test pattern.

## Verification

```bash
go test ./pkg/ui/components/editor/ -run TestFind -v
```
