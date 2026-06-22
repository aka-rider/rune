# ADR-0009 — In-File Live Search

**Status:** Accepted  
**Date:** 2026-06-22

---

## Context

Users need to locate text within the document they are editing. The design goal was:

- `Cmd+Shift+F` / `^F` opens a full-width search bar below the title row
- Typing highlights all matches live (dim) and shows a count
- Enter / Shift+Enter jumps to the next / previous match nearest the cursor
- `⌘G` / `⇧⌘G` repeat navigation even after the bar is closed
- `↑` / `↓` in the bar recall prior searches with fuzzy filtering
- SQLite-backed global history, dedup-upsert, capped at 200 entries
- Case-insensitive fixed-string matching; no regex, no replace in v1

---

## Decision

### Match computation: `pkg/editor/search`

A new package performs all text matching, independent of the UI. `Find` walks
the content rune-by-rune comparing via `unicode.ToLower` (without modifying
the buffer), and returns non-overlapping byte-offset `Span` pairs. `FuzzyMatch`
implements subsequence matching used for history recall only.

### Match state ownership: `textedit.Model`

Per the **State Residency Rule** (§2.1), the component that renders match
highlights must own the match state. `textedit.Model` gained six fields:
`searchMatches`, `searchActive`, `searchQuery`, `searchCaseInsensitive`, and
`searchRev`. The public API (`SetSearchQuery`, `FindNext`, `FindPrev`,
`ClearSearch`, `SearchMatches`, `SearchActive`, `MatchCount`) lives in
`search_state.go`.

`markdownedit.Model` shadows all four model-returning methods to preserve value
semantics (embedding promotion would return `textedit.Model`, not
`markdownedit.Model`).

### Visibility filtering

Matches inside hidden markdown syntax (bold delimiters, URL parts, etc.) have no
rendered cells and cannot be scrolled to. `filterVisibleMatches` walks the
display snapshot's `Slice(0, TotalRows)`, marking byte offsets covered by
`Revealed` spans directly, and by `CellMap.BufOffset` for `Rendered` spans.
Only matches with at least one visible byte survive.

### Cell overlay: `cell.go`

`Cell` gained `Match bool` and `ActiveMatch bool` fields. `ApplyMatchOverlay`
sets them by scanning `BufOffset` against the match list. `cellEffectiveStyle`
applies match styles with precedence: Cursor > ActiveMatch > Selected > Match.
The signature of `CellsToString` extended to five arguments; callers that do
not participate in search (image placeholder cells, single-line textedit uses)
pass `lipgloss.NewStyle()` for the two new style arguments.

Faint rendering of the unfocused editor is suppressed when search matches are
present so highlights remain visible.

### Stale revision guard

`FindNext` / `FindPrev` check whether the buffer revision has advanced since
the last `SetSearchQuery` call. If so they re-run `SetSearchQuery` before
navigating, preventing stale byte-offset jumps after edits.

### Search bar component: `pkg/ui/components/search`

A new single-line textedit-backed component emits `SubmitMsg` (Enter / Shift+Enter)
and `CloseMsg` (Escape). Width is computed from `SetSize` minus prompt and status;
the field never calls `SetRect` from `View()`. History navigation (↑/↓) is a
state machine over a lazily-computed working set, filtered via `FuzzyMatch` when
a draft is present.

### Workspace pane: `paneSearch`

A new pane `paneSearch = 5` was added. It satisfies `isCenter()` so the center
pane border stays active while the bar has focus. `recalcLayout` subtracts the
search bar's height (0 or 1) from `editorH` and shifts the editor's Y offset
accordingly.

`FindOpen` (global, Priority 3) opens the bar and routes focus to `paneSearch`.
The bar query change drives `SetSearchQuery`; `SubmitMsg` drives
`FindNext`/`FindPrev` and persists the query asynchronously. `CloseMsg` (Escape)
calls `ClearSearch` and returns focus to `paneCenter`.

`FindNext` / `FindPrev` bindings (⌘G / ⇧⌘G) are handled globally so they
work with the bar closed.

### History persistence: `docstate.search_history`

A `search_history` table (`query TEXT PRIMARY KEY, last_used_at TEXT NOT NULL`)
was added to the permanent schema via `CREATE TABLE IF NOT EXISTS`. No
`user_version` bump is required. `AppendSearchQuery` upserts and then prunes
to 200 rows. `SearchHistory` returns entries most-recent-first. The store is
loaded asynchronously on `StoreReadyMsg` and delivered via `historyLoadedMsg`.

### Keymap change

`FindOpen` dropped the `"super+f"` binding (plain ⌘F). The retained bindings
are `"shift+super+f"` (⇧⌘F) and `"ctrl+f"` (^F), matching the plan spec and
avoiding a collision with a potential future browser-style find.

---

## Consequences

- Matches inside hidden markdown syntax are intentionally excluded from
  navigation. A user searching inside `**bold**` will not land on the `**`
  delimiters in rendered mode. Matches remain visible when the cursor is on
  that line (Revealed mode) and will be navigable then.
- `WantsModalInput` on `textedit.Model` now always returns `false`. The
  Priority-4 modal-capture block in `workspace_update_keys.go` was removed.
- The `findOverlay.go` file is retained (empty) to avoid disrupting any
  external tooling that may reference the filename; its type and method are gone.
