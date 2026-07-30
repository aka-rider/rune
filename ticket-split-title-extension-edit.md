# Split extension into separate focusable subcomponent in title field

Groomed at: 47c3e13 (2026-07-30T00:00:00Z)

## Source

From `TODO.md`:

> Title component should allow to change file extension. So for that extension should be visible as a separate subcomponent or separate part of the title component, and to focus the extension user should explicitly press right arrow.

## End Goal

The title field renders as two distinct, visually separable subcomponents: the stem and the extension. When the title has focus, the cursor defaults to the stem. Pressing Right arrow at the end of the stem moves the cursor into the extension subcomponent, allowing the user to edit it. The extension is rendered as a separate visual part (distinct style or background) so the user can see where the stem ends and the extension begins.

## Verify first

1. `grep -n 'MARKDOWN_EXT' crates/rune-tui/src/title.rs` — confirms `MARKDOWN_EXT` is the single hardcoded extension constant. Expected: line 42, `pub const MARKDOWN_EXT: &str = "md";`
2. `grep -n 'MARKDOWN_EXT' crates/rune-tui/src/rename.rs` — confirms `target_path` appends `.md` unconditionally. Expected: line 247, `parent.join(format!("{stem}.{}", title::MARKDOWN_EXT))`
3. `grep -n 'field_spans\|TitleField' crates/rune-tui/src/title.rs` — confirms current rendering is a single monolithic text field. Expected: `field_spans` at line 261 renders `field.text` as one unit.
4. `git log --oneline -3 -- crates/rune-tui/src/title.rs` — confirms no recent changes that would have already addressed this. Expected: latest is `a73faf0` (docs cleanup).

Tripwire: if any of these checks fail (e.g., `MARKDOWN_EXT` no longer exists, or the title field already has extension support), STOP — the ticket is stale; re-groom.

## Done when

- `crates/rune-tui/src/title.rs` renders the extension as a visually distinct subcomponent in both focused and unfocused modes (unfocused: `stem.ext`; focused: `stem` + `ext` with separate styling)
- `TitleField` tracks cursor position independently for stem and extension; Right arrow at end of stem moves cursor into extension
- `TitleField` tracks extension text separately from stem text (not implicitly derived)
- `target_path` in `crates/rune-tui/src/rename.rs` uses the extension from `TitleField` instead of hardcoding `MARKDOWN_EXT`
- `is_valid_stem` is extended or a new `is_valid_extension` validates extension input (rejects invalid characters like `/`, `\0`, etc.)
- Left arrow at start of extension moves cursor back into stem
- `Esc` reverts both stem and extension to their committed values
- All existing tests in `crates/rune-tui/src/title.rs` and `crates/rune-tui/tests/title_breadcrumb.rs` continue to pass
- New tests cover: cursor motion across stem/extension boundary, editing extension, reverting extension, rendering with extension in focused/unfocused modes

## Open Questions

1. **Should the extension be editable to arbitrary values, or should rune enforce `.md`?**
   - The document system (`document.rs` line 38-50) already supports arbitrary extensions — `.md` maps to `Markdown`, recognized extensions map to `Code` (via `rune_ts::lang::resolve`), anything else is `Plain`.
   - Recommended: allow arbitrary extensions. The underlying system already handles them correctly.
   - If the team wants to enforce `.md`, add validation in `is_valid_extension` that only accepts `"md"` (case-insensitive).

2. **Should the extension be visually styled differently from the stem?**
   - Recommended: yes. A distinct style (e.g., dimmed/subtle color) signals to the user that the extension is a separate, editable part while maintaining visual hierarchy.
   - The current `field_spans` function returns `Vec<Span<'static>>` — adding a second styled span for the extension is a natural fit.

3. **Should the Go reference implementation be updated in parallel?**
   - The Go implementation (`golang/pkg/ui/pages/workspace/workspace_update.go` lines 73, 82, 235) also hardcodes `.md`.
   - Recommended: groom a separate ticket for the Go port. The Rust implementation can lead since it is the active development target.

4. **What happens when a file is renamed from `.md` to a non-markdown extension?**
   - The `DocumentKind` derivation (`document.rs` line 38-50) will automatically pick the correct kind based on the new extension. No additional work needed.

5. **Should the extension be shown in the breadcrumb as well?**
   - The breadcrumb (`crates/rune-tui/src/breadcrumb.rs`) already shows the full file name including extension. No change needed.

## Facts

- **Current architecture**: `TitleField` (title.rs:59-68) holds a single `text: String` (the stem) and a byte-offset `cursor: usize`. The extension is never part of `text`.
- **Extension source**: `MARKDOWN_EXT` constant (title.rs:42) is the single hardcoded `.md` extension. It is used by:
  - `rename.rs:247` in `target_path(from, stem)` → `parent.join(format!("{stem}.{}", title::MARKDOWN_EXT))`
  - `rename.rs:553` in `bind_new` → `dir.join(format!("{stem}.{}", title::MARKDOWN_EXT))`
- **Stem extraction**: `stem_for(doc)` (title.rs:143-149) uses `doc.file_path.as_ref().and_then(|p| p.file_stem())` — the standard library's `file_stem()` which strips the extension.
- **Rendering**: `field_spans` (title.rs:261-277) renders `field.text` as a single span sequence split only at the cursor position.
- **Unfocused rendering**: `draw` (title.rs:240-255) calls `doc.file_name()` which returns the full file name including extension (from `Document::file_name()`).
- **Key handling**: `handle_key` (title.rs:170-205) moves cursor within `field.text` bounds only. Right arrow at end of text does nothing.
- **Rename workflow**: `rename.rs:179-241` (`begin`) reads `app.title.text` as the stem, appends `.md`, and calls `target_path`. The extension is never editable through the rename flow.
- **Focus trigger**: `dispatch.rs:416-418` — pressing Up at buffer top calls `pane::focus_title(app)`, which sets `app.focus = Pane::Title`.
- **Document kind system** (`document.rs:38-50`): supports arbitrary extensions. `.md` → `Markdown`, recognized → `Code`, other → `Plain`. No hard `.md` enforcement at the document level.
- **Go reference** (`workspace_update.go:73,82`): also hardcodes `.md` via `filepath.Join(dir, msg.Name+".md")`. No extension editing in Go either.
- **Test file**: `crates/rune-tui/tests/title_breadcrumb.rs` tests title row rendering at the integration level (uses `TestBackend`).
- **Existing title tests**: `crates/rune-tui/src/title.rs:279-375` — unit tests for `TitleField` cursor motion, deletion, stem validation, dirty dot, focused field rendering.
