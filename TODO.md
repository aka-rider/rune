## Open

- [ ] Title component should allow to change file extension. So for that extension should be visible as a separate subcomponent or separate part of the title component, and to focus the extension user should explicitly press right arrow.
- [ ] Rust implementation should follow Golang implementation. in separation of rich text editor and verbatim editor. verbatim raw text editor should be used as a tile editor component so that selection, undo (w/o persistence), word movement would also work while editing the title.

## File-size budget (§1.6)

- [ ] `crates/rune-tui/src/explorer.rs` is over the 500-line budget — it was already at 613 lines before the `..` parent-row change pushed it to 650. Splitting it was deliberately out of scope for that change, so the split is deferred here.
- [ ] `crates/rune-tui/tests/opentabs.rs` is now 571 lines, over the budget — the global `^w`/`^1`-`^0` binding tests were appended to the existing tabs test file rather than split out. Decompose it (e.g. a separate `opentabs_global.rs`) next time it is touched.

## Parked tickets

Groomed, verified against the tree, and deliberately not worked — each needs its own plan.

- **`ticket-use-verbatim-editor-title.md`** — the title field should reuse a verbatim text-editing component (selection, word movement, copy/paste, select-all), the way Go's `title.Model` wraps `textedit.Model`. Parked because no such abstraction exists anywhere in the Rust workspace: it means extracting a reusable editable core (buffer + cursor set + command dispatch + a sanitize seam) out of `Document`, plus its own minimal render path and clipboard-message routing. Not a batch-sized change.
- **`ticket-split-title-extension-edit.md`** — make the file extension a separately focusable part of the title field. Parked because it is blocked on the verbatim-editor work above (the ticket's own open questions concede this). Building it first would add a second hand-rolled two-field cursor model to `title.rs` — exactly the debt the other ticket removes.

## Closed without action

- **`ticket-hide-cursor-unfocused-editor.md`** — the end goal already holds. `Document::shows_caret()` is the single predicate, `apply_cursor_overlays` early-returns on it (gating the caret *and* the selection highlight together), the fuzz snapshot carries `caret_visible`, the `CUR-NO-CARET-HIDDEN` invariant fails any REVERSED cell rendered while the caret is hidden, and `crates/rune-tui/tests/tui_render.rs` pins all three cases.
