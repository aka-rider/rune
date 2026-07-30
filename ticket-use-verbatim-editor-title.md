# Use verbatim editor as title component

Groomed at: 47c3e13 (2026-07-30T00:00:00Z)

## Source

From `TODO.md`:

> Rust implementation should follow Golang implementation. in separation of rich text editor and verbatim editor. verbatim raw text editor should be used as a tile editor component so that selection, undo (w/o persistence), word movement would also work while editing the title.

## End Goal

The title field (`crates/rune-tui/src/title.rs`) uses the same verbatim text-editing component that the Go implementation uses (`golang/pkg/ui/components/textedit`), giving it selection (Shift+arrows), word movement (Alt+Left/Right), undo (Cmd+Z), copy/paste (Cmd+C/V), and select-all (Cmd+A) — matching Go's `title.Model` which wraps `textedit.Model` with `WithSingleLine()`.

## Verify first

1. `grep -n "textedit.Model" /Users/xiii/Developer/rune/golang/pkg/ui/components/title/title.go` — should show `field textedit.Model` on line 27
2. `grep -n "textedit.New" /Users/xiii/Developer/rune/golang/pkg/ui/components/title/title.go` — should show `textedit.New(keys, st, allOpts...)` on line 38 with `textedit.WithSingleLine()` in `allOpts`
3. `grep -n "pub struct TitleField" /Users/xiii/Developer/rune/crates/rune-tui/src/title.rs` — confirms current hand-rolled `TitleField` (line 59)
4. `grep -c "WordLeft\|WordRight\|SelectChar\|SelectWord\|SelectAll" /Users/xiii/Developer/rune/crates/rune-tui/src/commands/nav.rs` — confirms these commands exist in Rust (should be > 0)
5. `grep -c "WordLeft\|WordRight" /Users/xiii/Developer/rune/crates/rune-tui/src/keymap/editor_bindings.rs` — confirms Alt+arrow bindings exist (should be > 0)

Tripwire: if any check fails, STOP — the ticket is stale; re-groom, don't work it.
If the end goal already holds (title field already delegates to a verbatim editor), close without action.

## Done when

1. `cargo test --package rune-tui -- title::tests` — all existing title tests pass (they should, since the public API surface of `TitleField` is preserved)
2. `cargo test --package rune-tui` — full crate compiles and passes
3. The `TitleField` struct in `title.rs` contains a verbatim editor field (not just `text: String, cursor: usize`)
4. The title's `handle_key` delegates to the verbatim editor for editing keys (CharLeft, CharRight, WordLeft, WordRight, SelectCharLeft, SelectCharRight, SelectWordLeft, SelectWordRight, Undo, Redo, Copy, Paste, SelectAll) rather than handling them in a big match statement
5. Invalid filename characters (`/`, `\`, `:`, `*`, `?`, `"`, `<`, `>`, control chars) are still filtered from input (inherited from Go's `filterFileName`)
6. The title is single-line: newline insertion is disabled (Go uses `textedit.WithSingleLine()`)
7. `Esc` still reverts to the committed name and returns focus to Editor
8. `Enter`/`Down` still commits the name and returns focus to Editor
9. `Cmd+C` / `Cmd+V` still work for copy/paste in the title field (the Go title handles `tea.ClipboardMsg` and `tea.PasteMsg` with filename filtering)
10. The command `git log --oneline -3 -- crates/rune-tui/src/title.rs` shows nothing after the groomed-at sha (i.e., the ticket hasn't been worked yet)

## Open Questions

1. **Architecture: extract vs. wrap.** The Go `textedit.Model` is a full-fledged component with buffer, cursors, display pipeline, viewport, syntax map, image handling, search, etc. The title only needs the bare text-editing core (buffer + cursors + command dispatch + single-line mode). Two options:
   - (a) Extract a `VerbatimEditor` type from `Document`'s editing machinery that wraps `Buffer` + `CursorSet` + command dispatch, without the display pipeline/viewport/syntax. This is the cleanest path but requires identifying what can be extracted from `Document`.
   - (b) Create a new minimal `VerbatimEditor` in `title.rs` (or a sibling module) that mirrors what `textedit.Model` provides minus the markdown/display machinery.
   - **Recommended:** (a) — look at `Document`'s fields and extract the editable-core subset (buffer, cursors, pending edits) into a reusable type. The display/render pipeline stays on `Document`.

2. **Undo persistence.** The Go title has no separate undo stack — the comment in `title.go`'s `handleKey` says: "⌘Z while renaming undoes the DOCUMENT (not the in-progress rename text) and yanks focus back to the editor." So the Rust title should similarly NOT have persistent undo. Cmd+Z should undo in the document (not the title text). This means the verbatim editor for the title should have NO journal — edits are transient, only committed via `Enter`/`Down`.
   - **Recommended:** The title's verbatim editor tracks edits in a transient buffer but does NOT push to `Journal`. `Esc` discards them entirely; `Enter`/`Down` commits by triggering `rename::begin`.

3. **How to handle filename filtering.** The Go title's `handleKey` filters invalid filename chars by replacing them (paste: replace with `_`, typed: drop silently). The Rust `TitleField` currently rejects invalid chars at the `handle_key` match arm level (the `!INVALID_NAME_CHARS.contains(&ch)` guard). When delegating to a verbatim editor, the editor will receive raw text input. Two approaches:
   - (a) Wrap the verbatim editor's `SanitizeFunc` seam (Go has `SanitizeFunc` on `textedit.Model`) to filter invalid filename chars before insertion.
   - (b) Pre-filter keystrokes in the title's `handle_key` before delegating to the verbatim editor.
   - **Recommended:** (a) — mirrors Go's `WithSanitizeFunc` pattern. The verbatim editor should accept a `SanitizeFunc` that the title sets to `filter_filename`.

4. **Copy/Paste in the title.** The Go title handles `tea.ClipboardMsg` and `tea.PasteMsg` at the title level (not delegating to textedit), filtering clipboard content through `filterFileName` before passing to the field. The Rust clipboard system uses OSC 52 writes and `pbpaste` reads. The title needs to:
   - Forward `Msg::ClipboardRead` (pbpaste result) to the verbatim editor with filename filtering
   - Forward `Msg::Paste` (bracketed paste) similarly
   - **Recommended:** Add clipboard message handling in the title's key/message dispatch that filters content through `filter_filename` before inserting into the verbatim editor.

5. **Rendering.** The current `TitleField` renders itself via `field_spans` in `title.rs`. When using a verbatim editor, the rendering path changes — the Go title delegates to `m.field.View()` when focused. The Rust `Document` renders via `render/mod.rs` which uses `View` snapshots. A verbatim editor for the title would need its own minimal render path (just text + cursor cell, no syntax highlighting, no images).
   - **Recommended:** The verbatim editor exposes a `render(&self, area: Rect, frame: &mut Frame)` method that draws text with cursor, similar to the current `field_spans` but driven by the editor's cursor state.

6. **Scope boundary with TODO.md line 4.** The adjacent TODO ("Title component should allow to change file extension") is a separate feature. This ticket only addresses reusing the verbatim editor for title editing. The extension-editing TODO can be built on top of the new architecture.

## Facts

**Go architecture (reference implementation):**
- `golang/pkg/ui/components/title/title.go`: `title.Model` contains `field textedit.Model` (line 27)
- `golang/pkg/ui/components/textedit/textedit.go`: `textedit.Model` has `buf buffer.Buffer`, `cursors cursor.CursorSet`, `pendingEdits []buffer.AppliedEdit`, command `registry`, keybind `resolver`, `singleLine` flag, `sanitizeFunc`
- `textedit.New()` accepts `WithSingleLine()`, `WithSanitizeFunc()`, `WithRegistry()`, `WithResolver()` options
- `title.New()` calls `textedit.New(keys, st, allOpts...)` where `allOpts` includes `WithSingleLine()` + registry/resolver
- `title.Update()` handles clipboard/paste messages at the title level (filtering through `filterFileName`), then delegates keypresses to `m.field.Update(msg)`
- `title.handleKey()` only handles title-specific keys: Enter/Down (commit + focus return), Escape (revert + focus return), and passes everything else to `m.field.Update()`
- `title.Commit()` emits `RenameRequestMsg` if text differs from `committed`
- `title.DrainEdits()`, `title.Cursors()`, `title.SetCursors()`, `title.ApplyInverse()`, `title.Reapply()` all delegate to `m.field`

**Rust current state:**
- `crates/rune-tui/src/title.rs`: `TitleField` is a hand-rolled struct with `text: String`, `cursor: usize`, `committed: String` (line 59)
- Methods: `seed()`, `revert()`, `accept()`, `set_text()`, `insert()`, `delete_left()`, `delete_right()`, `prev_boundary()`, `next_boundary()`
- `handle_key()` (line 170): big match statement handling Enter, Down, Escape, Left, Right, Home, End, Backspace, Delete, printable chars — ~40 lines of hand-coded key handling
- No word movement, no selection, no undo, no copy/paste, no select-all
- `crates/rune-tui/src/app.rs` line 112: `pub title: crate::title::TitleField` on `App`

**Rust commands available (already implemented, not wired into title):**
- `commands/nav.rs`: `word_left_offset`, `word_right_offset`, `select_all`, `escape` (collapse selection)
- `commands/edit.rs`: `insert_char`, `delete_left`, `delete_right`
- `commands/clipboard.rs`: `copy`, `cut`, `paste` (with OSC 52 and pbpaste)
- `keymap/editor_bindings.rs`: `WordLeft` (Alt+Left), `WordRight` (Alt+Right), `SelectCharLeft/Right` (Shift+Left/Right), `SelectWordLeft/Right` (Shift+Alt+Left/Right), `SelectAll` (Cmd+A), `Undo` (Cmd+Z), `Redo` (Cmd+Shift+Z), `Copy` (Cmd+C), `Paste` (Cmd+V), `Cut` (Cmd+X)
- `keymap.rs` `Command` enum (line 136): all the above commands are defined

**Rust Document (what the verbatim editor would be extracted from):**
- `crates/rune-tui/src/document.rs`: `Document` struct has `buffer: Buffer`, `cursors: CursorSet`, `journal: Journal`, `viewport: Viewport`, `doc: DocMachine`, `highlight: HighlightState`, etc.
- The editing-relevant subset: `Buffer` + `CursorSet` + command dispatch (resolver + registry) — the rest (DocMachine, Viewport, HighlightState, Journal) are display/persistence concerns
- `commands/edit_core.rs`: `commit_edit_batch` is the buffer-mutation chokepoint — pushes to journal, db, dirty cache. The title verbatim editor needs a version that skips the journal/db/dirty parts

**Constitution constraints:**
- §1.5: offsets are BYTES, display widths are terminal cells — the title uses byte offsets (current `cursor: usize` is already byte-based)
- §12: "the title field is unjournaled — a rename is one atomic bind" — the title must NOT push to the document journal
- §1.3: clamp edit ranges to live byte length — the verbatim editor's edit commands already do this via `Buffer::apply_edits`

**Files that would change:**
- `crates/rune-tui/src/title.rs` — replace `TitleField` with verbatim-editor-backed version
- `crates/rune-tui/src/app.rs` — no change to `title` field type (still `TitleField`, just different internals)
- `crates/rune-tui/src/dispatch.rs` — `Pane::Title` arm may need minor adjustment if key routing changes
- Potentially new file: `crates/rune-tui/src/verbatim.rs` or similar for the extracted verbatim editor core
- Potentially new file: `crates/rune-tui/src/render/verbatim.rs` for minimal verbatim rendering
