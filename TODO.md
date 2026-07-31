## Open

- [ ] Title component should allow to change file extension. So for that extension should be visible as a separate subcomponent or separate part of the title component, and to focus the extension user should explicitly press right arrow.
- [ ] Rust implementation should follow Golang implementation. in separation of rich text editor and verbatim editor. verbatim raw text editor should be used as a tile editor component so that selection, undo (w/o persistence), word movement would also work while editing the title.
- [ ] The title field has no horizontal scroll (title/rename plan assumption A1): an over-long file name is clipped by `Paragraph`, not scrolled to follow the cursor, so editing the tail of a name wider than the terminal is awkward. A viewport can be added to `TextField` later; not attempted here to avoid desyncing byte offsets from a truncated string.

## File-size budget (§1.6)

A batch of twelve splits landed: all six `rune-db` sources, `rune-tui`'s
`save.rs`/`document.rs`/`explorer.rs`, `tests/opentabs.rs`, and the two worst
test files (`conceal_roundtrip.rs` at 1453 lines and `tests/highlight.rs`).
`explorer.rs` and `opentabs.rs` — the two previously recorded here — are done.

A second, much larger batch (the `split(...)`/`refactor(...)` work packages
that followed, WP A through H) then split every remaining test file and
most remaining sources that used to be listed here: `db_wiring.rs`,
`rename.rs`, `tui_render.rs`, `explorer.rs` (test), `multiprocess.rs`,
`tripwire.rs`, `table_render.rs`, `main.rs`, `buffer.rs`, `wrap/mod.rs`,
`nav/lib.rs`, `keymap/index.rs`, `breadcrumb.rs`, `editor_bindings.rs`,
`driver/mod.rs`, `runtime/mod.rs`, `commands/nav.rs`, `commands/edit_lines.rs`,
`keymap.rs`, `dispatch.rs`, `footer.rs`, and `table/layout.rs` are all under
the ceiling again. A handful remain over it, mostly the residue of the same
long-running debt:

- [ ] Re-measured against the tree after the title/rename work merged, because
  both sides of that merge carried stale counts: `rune-tui/src/app.rs` (550),
  `rune-md/src/emit/mod.rs` (536), `rune-tui/src/rename.rs` (526),
  `rune-syntax/src/wrap/mod.rs` (520), `rune-fuzz/src/generate/palette.rs`
  (517), `rune-syntax/src/syntax.rs` (505), `rune-tui/src/document.rs` (501).
  `rune-tui/tests/rename_bind.rs`, previously listed here at 795 lines, is
  DONE: split (plan WP5) into `rename_bind.rs` (373 — focus/typing, the
  end-to-end no-store rename, draft naming), `rename_refusals.rs` (136),
  `rename_gate.rs` (201 — the extension gate plus the field's own
  word-motion/selection/undo editing), and `rename_clipboard.rs` (152),
  the same way an earlier batch split `rename.rs` itself; all four pull
  shared fixtures from `rename_common`.

Two of those grew slightly in an earlier batch and are recorded per the house
rule: `dispatch.rs` 513 → 527 (the span-cap truncation status branch, since
brought back under budget by the later split that moved `handle_db_event`
into `db_dispatch.rs`) and `db_wiring.rs` 875 → 909 (the pending-op sweep
regression test, since split into `db_wiring_degraded.rs`/`_hydrate.rs`/
`_lifecycle.rs`). Both were already over budget beforehand.
`commands/edit_core.rs` did cross the ceiling when its no-op-filter tests
landed and was split the same day, so it was never on the list.
`rune-fuzz/src/generate/palette.rs` crossed it the same way in plan WP5:
468 → 517 for `TITLE_MOTION_KEYS`, the five-entry palette `cluster_chrome`
pairs with `CTRL_R_KEY` so a single generated cluster both parks focus on
the title and exercises one of its own word-motion/selection/undo
bindings. Not split further in the same batch — pulling one five-entry
array into its own file over a 17-line overage would be its own drive-by.

The WP2 focus-chokepoint refactor (title-editing plan) grew `app.rs` and
`rename.rs` further, both already over budget beforehand: `app.rs` 524 → 588
(the private `focus` field plus its three writers —
`focus_title`/`refocus_title`/`set_focus`/`blur_title` — the whole point of
the change, so splitting them out defeats the "one writer" invariant they
exist to enforce) and `rename.rs` 632 → 663 (`begin`'s six-way refusal
enumeration now returns `Commit` instead of `bool`, each arm's reasoning
spelled out per decision 7). Integrating that work alongside `rr`'s own
`rename.rs` split (`rename_bind.rs`/`rename_collision.rs`/`rename_replace.rs`)
pulled `bind_new`/`create_cmd` out into the pre-existing `rename_create.rs`
sibling, landing `rename.rs` at 523 — still over, but the smallest it has
been since WP2 started, and not a candidate for a second split mid-merge
(the `begin`/`apply_outcome`/`replace_confirmed` state-machine drive is one
coherent unit). `app.rs` stays at 538: the four focus methods are the single
chokepoint decision 8 exists to create, and splitting them apart would
recreate the multiple-writer hazard this refactor removes. `tests/rename.rs`'s
own eleven new regression tests (WP2.S11, including the ordering guard for
decision 8) were never written into a 1196-line monolith — this integration
merge relocated them straight into `rr`'s already-split `rename_bind.rs`
(the read-only-title refusal) and a new sibling `rename_focus.rs` (the
other ten), both comfortably under the ceiling.

The single most-deferred item remains `app.rs`'s `handle_key` /
`handle_editor_key` / `handle_db_event` extraction, deferred across nine
consecutive work packages.

WP2 (`LineDecor` model + emit population, plan "markdown line decoration")
pushed three more files over or further over the ceiling: `rune-md/src/emit/
walk.rs` 515 → 539, since brought back under budget by its own later split
into `walk_inline.rs`; `rune-md/src/emit/mod.rs` 499 → 535 (the new
`emit_with`/`EmitOut::icons`/`EmitOut::decors` plumbing the 3-arg `emit` now
wraps); and `rune-syntax/src/syntax.rs` 499 → 505 (the new
`SyntaxLine::decor` field and its doc comment). New logic went into new
sibling files (`emit/decor.rs`, `emit/decor_tests.rs`, `rune-syntax/src/
decor.rs`) per the plan's own instruction; only wire-up lines touched
`emit/mod.rs` and `syntax.rs`, but that was still enough to cross or extend
the ceiling, and neither has been split since.

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

- **`ticket-hide-cursor-unfocused-editor.md`** — the end goal already holds. `Document::shows_caret()` is the single predicate, `apply_cursor_overlays` early-returns on it (gating the caret *and* the selection highlight together), the fuzz snapshot carries `caret_visible`, the `CUR-NO-CARET-HIDDEN` invariant fails any REVERSED cell rendered while the caret is hidden, and `crates/rune-tui/tests/tui_render_focus.rs` pins all three cases.
