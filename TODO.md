## Open

- [ ] Title component should allow to change file extension. So for that extension should be visible as a separate subcomponent or separate part of the title component, and to focus the extension user should explicitly press right arrow.
- [ ] Rust implementation should follow Golang implementation. in separation of rich text editor and verbatim editor. verbatim raw text editor should be used as a tile editor component so that selection, undo (w/o persistence), word movement would also work while editing the title.

## File-size budget (§1.6)

A batch of twelve splits landed: all six `rune-db` sources, `rune-tui`'s
`save.rs`/`document.rs`/`explorer.rs`, `tests/opentabs.rs`, and the two worst
test files (`conceal_roundtrip.rs` at 1453 lines and `tests/highlight.rs`).
`explorer.rs` and `opentabs.rs` — the two previously recorded here — are done.

Twenty-seven files remain over the ceiling. None was introduced by that batch;
they are the residue of the same long-running debt, listed here so the campaign
is visible rather than rediscovered file by file.

- [ ] Test files: `rune-tui/tests/db_wiring.rs` (909), `rune-tui/tests/rename.rs` (804), `rune-db/tests/multiprocess.rs` (803), `rune-tui/tests/tui_render.rs` (698), `rune-fuzz/tests/tripwire.rs` (595), `rune-md/tests/table_render.rs` (590), `rune-tui/tests/explorer.rs` (523).
- [ ] Sources: `rune-cli/src/main.rs` (801), `rune-core/src/buffer.rs` (689), `rune-tui/src/db.rs` (645), `rune-tui/src/rename.rs` (632), `rune-syntax/src/wrap/mod.rs` (616), `rune-nav/src/lib.rs` (595), `rune-tui/src/keymap/index.rs` (572), `rune-tui/src/breadcrumb.rs` (557), `rune-tui/src/keymap/editor_bindings.rs` (553), `rune-fuzz/src/driver/mod.rs` (553), `rune-tui/src/runtime/mod.rs` (549), `rune-tui/src/commands/nav.rs` (546), `rune-tui/src/commands/edit_lines.rs` (543), `rune-tui/src/keymap.rs` (528), `rune-tui/src/dispatch.rs` (527), `rune-tui/src/app.rs` (524), `rune-md/src/emit/walk.rs` (509), `rune-tui/src/footer.rs` (506), `rune-md/src/table/layout.rs` (501).

Two of those grew slightly in this batch and are recorded per the house rule:
`dispatch.rs` 513 → 527 (the span-cap truncation status branch) and
`db_wiring.rs` 875 → 909 (the pending-op sweep regression test). Both were
already over budget beforehand. `commands/edit_core.rs` did cross the ceiling
when its no-op-filter tests landed and was split the same day, so it is not on
the list.

The single most-deferred item remains `app.rs`'s `handle_key` /
`handle_editor_key` / `handle_db_event` extraction, deferred across nine
consecutive work packages.

- [ ] Two files landed within a few lines of the ceiling and will breach on the next small edit: `rune-db/src/writer.rs` (497) and `rune-db/src/materialize.rs` (496). Whoever touches either next should take the split rather than squeeze under.
- [ ] The `rune-db` splits copy their test scaffolding rather than share it — `open()`, `insert_test_document`, `Fixture`, `always_dead` and friends are now verbatim in both `rename_bind.rs` and `rename_replace.rs` (~50 lines), and similarly across the `writer_*`/`store_*` pairs. Note this predates the splits as a crate-wide habit (`open()` alone is defined in sixteen files), so the fix is one `#[cfg(test)]` support module for the whole crate — the pattern `conceal_common`/`opentabs_common`/`highlight_common` already use on the test side — not a per-split patch.

## Parked tickets

Groomed, verified against the tree, and deliberately not worked — each needs its own plan.

- **`ticket-use-verbatim-editor-title.md`** — the title field should reuse a verbatim text-editing component (selection, word movement, copy/paste, select-all), the way Go's `title.Model` wraps `textedit.Model`. Parked because no such abstraction exists anywhere in the Rust workspace: it means extracting a reusable editable core (buffer + cursor set + command dispatch + a sanitize seam) out of `Document`, plus its own minimal render path and clipboard-message routing. Not a batch-sized change.
- **`ticket-split-title-extension-edit.md`** — make the file extension a separately focusable part of the title field. Parked because it is blocked on the verbatim-editor work above (the ticket's own open questions concede this). Building it first would add a second hand-rolled two-field cursor model to `title.rs` — exactly the debt the other ticket removes.

## Recently closed

- **Zero-width edit batches no longer dirty a clean file** — the commit chokepoint drops edits that change nothing, in both the Rust and Go implementations.
- **`build_5k_doc` has one source of truth** — the bench and perf-guard copies had already silently diverged (the guard measured a table-bearing document the bench did not).
- **`Mem::stat` and `read_dir` share one synthetic-directory predicate.**
- **Span-cap truncation surfaces a status line**, with timeout outranking it when both hold.
- **`make test-fuzz` no longer dirties a tracked file** — proptest persistence moved under the gitignored artifacts directory, so the gate list is idempotent.
- **`db_ops`/`db_load_versions` merged into one `PendingOp`** — the two maps were swept separately, leaking load versions on document close; one value makes that unreachable.
- **Rename displaced-bytes attribution audited: NOT reachable.** The observation is attributed to the renaming document and carries `origin='swap'`, which ancestor selection's `origin IN ('load','save','resolve')` filter excludes outright. Pinned by a test seeded to fail if either guard were removed.
- **Two of the three comrak strict-invariants repros are closed** by the lone-`\r` shadow-copy parse, re-verified under the feature flag. The third still panics, isolating the surviving cause to tab-stop expansion alone.

## Closed without action

- **`ticket-hide-cursor-unfocused-editor.md`** — the end goal already holds. `Document::shows_caret()` is the single predicate, `apply_cursor_overlays` early-returns on it (gating the caret *and* the selection highlight together), the fuzz snapshot carries `caret_visible`, the `CUR-NO-CARET-HIDDEN` invariant fails any REVERSED cell rendered while the caret is hidden, and `crates/rune-tui/tests/tui_render.rs` pins all three cases.
