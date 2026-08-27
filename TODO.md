# TODO — Refactor Ledger

Reinstated 2026-08-07 from a three-audit sweep of the codebase. This is an
inventory of known violations of `CONSTITUTION.md`'s governing rules and of
non-idiomatic implementations with better alternatives available; it seeds a
future refactor plan. New code must not repeat these patterns even where
legacy code still does. Fixes land with tests per the constitution, and an
entry is deleted in the same commit that fixes it.

## Bug-hunt sweep (2026-08-26)

Product of a parallel multi-crate bug hunt (eight review passes, one per
subsystem plus a cross-cutting security sweep). These are *defects*, not
style — each carries a concrete failure scenario and a confidence tag.
`confirmed (executed)` means the reviewer built a repro and ran it;
`confirmed` means fully traced in source; `plausible` means the mechanism
is certain but one link (usually a live user sequence) was not reproduced.
Entries are ordered by severity. Fixes land with a regression test per the
constitution and the entry is deleted in the same commit.

### DATA-LOSS

### CRASH / DoS

### SECURITY

### CORRECTNESS

### MINOR


- **Concealed markdown delimiters can never take a background overlay** — `crates/rune-tui/src/render/cell.rs:147` (`paint_range`) keys purely on `Cell::buf_offset`, and `crates/rune-md/src/emit/walk_inline.rs:145,152` (`hide_range`) emits no cell at all for unrevealed link/image markup, so the offsets simply aren't on screen and the paint silently no-ops. It bites the matching-bracket highlight hardest: in a markdown document the brackets a user actually looks at are `[`…`](url)`, and when only one endpoint's element is `Revealed` the partner cannot be lit. Search and selection share the mechanism and the same blind spot. Needs a decision at the rune-md conceal boundary (reveal the partner's element, or mark the nearest visible cell), not a render-layer patch.
- **The Kitty keyboard flags are pushed blind** — `crates/rune-tui/src/term.rs:135` sends `Csi::Keyboard(Keyboard::PushFlags(...))` unconditionally with no capability probe and no read-back, so rune cannot distinguish a terminal that honoured `DISAMBIGUATE_ESCAPE_CODES`/`REPORT_ALTERNATE_KEYS` from one that dropped the CSI on the floor. Every `secondary: true` alternate row in the binding tables is a guess about which of the two worlds we're in. A `CSI ? u` query would settle it and would let a one-time message name the terminal's own setting when a chord is unreachable.
- **`is_insertable_key_char` is default-allow** — `crates/rune-tui/src/dispatch.rs:324` is just `!ch.is_control()`, so any codepoint that reaches the unbound-key fallback at `dispatch.rs:305` is written into the user's document. That is how macOS Option-composed characters (`µ`, `∫`, `ƒ`) landed in documents while the ⌥ chords they were meant to trigger stayed dead. The chords have moved off ⌥, but the policy behind them is still "insert anything we don't recognise" — against the prime directive it deserves to be a deliberate allow-list rather than a fall-through.

## Architecture

### Session-driver migration residue
- **Where**: `crates/rune-tui/tests/rename_common/mod.rs` (kept App-layer fixtures: `seeded_vfs`, `app_with`, `app_with_store`, `unsaved_named_app_with_store`, `next_event`, the `wait_for_*` waits, `send`, `type_text`, `type_new_name`); its consumers are THIRTEEN test binaries, not six: `bind_new_named.rs`, `save_state_machine.rs`, `materialize_dead_writer_reentrancy.rs`, `materialize_fatal_two_docs.rs`, `refused_hydration_detach.rs`, `reading_view.rs`, `rename_gate.rs`, `rename_bind.rs`, `rename_refusals.rs`, `rename_collision.rs`, `rename_replace.rs`, `rename_clipboard.rs`, `rename_focus.rs`; `navhistory_common` (embeds `explorer_common` via `#[path]`, still builds bare `App`s) with consumers `navhistory.rs`/`navhistory_browse.rs`; `set_doc_db_for_test` (consumed by the kept fixtures, `materialize_fatal_two_docs.rs`, and `g7_shared_file_baseline.rs`).
- **Wrong**: the 2026-08-13 migration moved nine `*_common` modules onto `rune_fuzz::Session`, but these binaries still construct bare `App`s through the duplicated fixture layer the migration exists to delete, so `rename_common` carries both layers side by side.
- **Instead**: migrate the six binaries and `navhistory_common` onto `Session`, then delete the App layer and re-evaluate whether `set_doc_db_for_test` is orphaned. Known driver gaps that blocked full migration, to close in `rune-fuzz` first: no out-of-order db-op delivery through checked steps (`deliver_db` is oldest-first; `merge_common::deliver_op_unchecked` is the workaround); the redivergence tracker only learns of external writes via `Action::DivergeDisk`; `Effects::raw`/timer arming invisible through `Session`; `ReadDir`/`ReadFile` Cmds dropped by the driver; a single rename-Cmd slot; no targeted `ClipboardRead` action; `SAVE-INFLIGHT-SM` rejects the legitimate `bind_new_now` Enter-materialize flip.
- **Done when**: no test binary constructs an `App` through a `*_common` fixture that duplicates `Session`, and the driver gaps above are either closed or individually recorded as deliberate.

## Mechanical

### O(file) per keystroke is the deliberate design ceiling
- **Where**: `crates/rune-core/src/buffer/mod.rs`, `crates/rune-core/src/buffer/lineindex.rs`, `crates/rune-tui/src/commands/edit_core.rs`, `crates/rune-tui/src/materialize_ack.rs`; perf-guarded by `crates/rune-tui/tests/perf_guard.rs:92` (`keystroke_view_cost_under_budget_on_a_5k_line_code_document`)
- **Wrong**: full content copy + full line-index clone + full memcmp + journal clones per edit batch; does not scale past the guard fixture's size.
- **Instead**: a rope with the same value-semantics facade, if the ceiling is ever hit in practice.
- **Done when**: not currently actionable — record only; revisit if the perf guard's fixture size stops matching real documents.

### Files over 500 lines
- **Where** (recomputed from the live tree with `wc -l`; comment purge below will change these numbers):
  - `crates/rune-md/src/parse/block.rs` — 552 (newly over — a mutation-testing pass added a `#[cfg(test)] mod tests` covering `clone_kind_tag`'s dead-code arms, `ranges_overlap`, and the container-depth cap; split candidate: move that block to a sibling `block_tests.rs`, `#[path]`-included the way this same directory's `inline.rs`/`inline_tests.rs` already split)
  - `crates/rune-md/src/parse/mod.rs` — 543 (newly over — the same pass added tests for `options_without_frontmatter` and an unterminated-fence boundary case; split candidate: move the existing `#[cfg(test)] mod tests` block to a sibling `mod_tests.rs`, `#[path]`-included the same way)
  - `crates/rune-md/src/snapshot/mod.rs` — 515 (newly over — the same pass added `DisplaySnapshot::display_to_wrap`/`wrap_to_display` clamp tests and a `line_start_of` pin; split candidate: move the `#[cfg(test)] mod tests` block to a sibling `mod_tests.rs`, matching `image_rows.rs`'s own in-file test module for now, or extracting both to siblings together)
  - `crates/rune-cli/src/bootstrap_tests.rs` — 1119 (test file; split candidate unchanged: move the launch-image-first tests (`launch_image_first_*`) plus `CountingReadVfs` to a sibling `bootstrap_tests_image.rs`, `#[path]`-included from `main.rs` the way `rune-db`'s `load_tests.rs` is from `load.rs`)
  - `crates/rune-tui/src/explorer_preview/tests.rs` — 1083 (test file)
  - `crates/rune-tui/src/global.rs` — 943 (grew from 886: command-palette WP2's `TogglePalette` rows plus its chord-freedom and `from_termina` reachability tests)
  - `crates/rune-tui/tests/diff_view.rs` — 801 (test file; newly over — the diff-view plan's own test suite grew through WP3-WP8: layout/alignment/intraline tests plus WP6's verb/chord/click tests all landed here; split candidate: move the verb and chord tests, `take_theirs_makes_the_region_same_and_undoes_in_one_step` through `click_in_the_left_pane_moves_the_right_pane_caret_to_the_aligned_line`, to a sibling `diff_view_verbs.rs`, leaving the layout/alignment/intraline tests here)
  - `crates/rune-vfs/src/mem.rs` — 788 (the path-lexical free functions moved out to a sibling `path_util.rs`, dropping it from 893; still over — split candidate: move the fault-injection hooks (`fail_next`/`fail_after`/`mutate_after_stat`/`churning`/`resolve_failures` and their methods) to a sibling `mem_fault.rs`)
  - `crates/rune-tui/src/pane.rs` — 796 (grew from 770: command-palette WP3's `registry_refusal` helper plus the availability gate hoisted into the Merge/ToggleReadOnly/TogglePin arms)
  - `crates/rune-tui/src/runtime/mod.rs` — 731 (grew from 713: the chunked-transmit drain loop's `turn` extraction and pump wiring; split candidate: move the run-loop body plus `turn` to a sibling `run_loop.rs`, leaving Msg/Effects declarations here)
  - `crates/rune-fuzz/src/generate/palette/palette_input.rs` — 619 (split out of `generate/palette.rs` (was 766, now a `palette/mod.rs` re-export shim) into `palette/palette_doc.rs` (document corpus: `SEEDS`/`MARKDOWN_FRAGMENTS`/`PASTE_PALETTE`/`TYPE_PALETTE`) and this file (input corpus: key/chord palettes, `CMDPAL_*`); `palette_doc.rs` landed under threshold at 149, this one is still over; split candidate: move the `CMDPAL_*` constants plus `MERGE_KEY`/`MERGE_RESOLVE_KEYS` to a sibling `palette_cmdpal.rs`)
  - `crates/rune-db/src/schema.rs` — 743 (grew from 684: command-palette recents added the `command_history` table and its apply-reconcile test; split candidate: move the schema tests to a sibling `schema_tests.rs`)
  - `crates/rune-tui/src/document/mod.rs` — 683 (grew from 679: command-palette WP4's `kind_pinned` field; split candidate unchanged: move the `ReadOnly` enum plus its `impl` block, which don't depend on `Document`'s own fields, to a sibling `read_only.rs`)
  - `crates/rune-tui/src/db.rs` — 676 (grew from 668: the `LoadPurpose` a re-baseline `Load` carries; split candidate unchanged: move the `FileBinding`/`DocDb` type definitions to a sibling `db_types.rs`, keeping the `Db`/writer-bridge wiring here)
  - `crates/rune-tui/src/linemap.rs` — 663 (newly over — the typed-offsets/image-id rework (`8cdaaef3`) grew this)
  - `crates/rune-tui/src/app.rs` — 625 (grew from 619: command-palette WP2's `next_palette_gen`/`last_persisted_command`/`command_history_ops` fields)
  - `crates/rune-db/tests/multiprocess/scenarios.rs` — 617 (test file)
  - `crates/rune-tui/tests/rename_focus.rs` — 613 (test file)
  - `crates/rune-fuzz/src/script/mod.rs` — 611 (newly over — mouse-event action support (`b970c59c`) grew the script table)
  - `crates/rune-fuzz/src/driver/session.rs` — 613 (grew from 592: the diff-left boot call, the arm/clear accessors, and `pending_highlights`; earlier: already over at 533 before a boot()-splitting cleanup added the named `new_app`/`open_seed_document`/`live_session`/`panicked_session` substeps; split candidate: move `Session::boot` and its four helpers, plus the `Seed`/`new_state` plumbing they share, to a sibling `boot.rs`, leaving `Session`'s post-boot methods here)
  - `crates/rune-db/src/sync_tests.rs` — 592 (newly over — this is the sibling test module split out of `sync.rs` in this pass; the moved test block was already this size in-file, so it needs its own further split, no candidate identified yet)
  - `crates/rune-tui/src/filesearch/tests.rs` — 591 (test file)
  - `crates/rune-fuzz/src/generate/cluster.rs` — 722 (grew from 585: WP7's `cluster_cmdpal` family — `cmdpal_open` plus its eight `cluster_cmdpal_*` arms; split candidate: move the merge/highlight/multicursor/cmdpal cluster functions, `cluster_merge` through `cluster_cmdpal`, to a sibling `cluster_scenarios.rs`, leaving the simpler single-shape clusters and `arb_cluster` itself here)
  - `crates/rune-tui/src/workspace/mod.rs` — 567 (pushed over by the `resolve_or_report` chokepoint added alongside the `resolve` signature change; split candidate: move the `#[cfg(test)] mod tests` block to a sibling test module)
  - `crates/rune-tui/src/messages/mod.rs` — 559
  - `crates/rune-cli/src/db_bootstrap.rs` — 558 (split candidate unchanged: move `bootstrap_untitled_db`/`ScratchDoc`/`DbBootstrapUntitled`/`degrade_untitled` to a sibling `db_bootstrap_untitled.rs`, matching the crate's own `bootstrap_tests.rs` split-out-of-`main.rs` pattern)
  - `crates/rune-tui/src/rename.rs` — 556
  - `crates/rune-tui/tests/opentabs.rs` — 551 (test file; grew from 479 in the Session-driver migration — `session.app_mut()` call verbosity plus rustfmt re-wrapping; split candidate: move the tab-limit/eviction tests to a sibling `opentabs_limit.rs` sharing `opentabs_common`)
  - `crates/rune-db/src/scratch.rs` — 551 (newly over — scratch-draft session naming (`a544b7fe`) grew this)
  - `crates/rune-db/src/rename_replace.rs` — 551 (newly over — publish-mode plumbing (`91e391f0`) grew this)
  - `crates/rune-db/src/observation.rs` — 545 (split candidate: separate the observation row I/O — `scan_observation`, `insert_observation_row`, the query functions — from the stat-facts side — `StatFacts`, `ObservationMeta`, `stat_identity` — into a sibling `stat_facts.rs`)
  - `crates/rune-tui/src/save/materialize_tests.rs` — 536 (test file; newly over — split candidate: move `snapshot_due_with_the_current_generation_enqueues_a_snapshot`/`snapshot_due_with_a_stale_generation_is_ignored`, which exercise `handle_snapshot_due` from `materialize_ack.rs` rather than this file's own CAS/publish path, to a sibling test module)
  - `crates/rune-tui/tests/rename_common/mod.rs` — 530 (newly over — the publish-mode enum rework (`87bd9f9e`) grew the shared rename test helpers)
  - `crates/rune-core/src/buffer/mod.rs` — 530 (newly over — the typed-offsets/image-id rework (`8cdaaef3`) grew the buffer)
  - `crates/rune-db/src/reaper.rs` — 526 (newly over — the merge-session reap exemption (`3c24a826`) grew this)
  - `crates/rune-tui/tests/tui_edit.rs` — 516 (newly over — vertical-motion caret-resettle tests (`c13d70fc`) grew this)
  - `crates/rune-db/src/adopt.rs` — 516 (newly over — abandoned-resolve retention logic (`d8f69e9a`) grew this)
  - `crates/rune-tui/src/commands/edit.rs` — 514 (newly over — the selection-consumption fix (`d1543de5`) grew this)
  - `crates/rune-tui/src/field.rs` — 512 (newly over — the typed-offsets rework (`8cdaaef3`) grew this)
  - `crates/rune-md/src/catalogue.rs` — 512
  - `crates/rune-tui/src/render/overlay.rs` — 511 (newly over — the Option-typed cell-offset rework (`008b9adf`) grew this)
  - `crates/rune-md/src/parse/inline.rs` — 511 (newly over — the forward-scan code-span fix (`78abf7a0`) grew this)
  - `crates/rune-fuzz/src/script/decode.rs` — 578 (grew from 509: WP7's `parse_palette_recents` multi-line action decoder)
  - `crates/rune-tui/src/db_enqueue.rs` — 554 (grew from 542: threading a persisted `EditKind` through `append_edit`/`send_append`/`replay_pending`/`rebase_move`; split candidate: move `resolve_drift` plus its doc comment to a sibling `db_drift.rs`, leaving the enqueue/`move_undo_pos`/`rebase_move` wiring here)
  - `crates/rune-db/src/snapshot.rs` — 610 (newly over — the persisted `EditKind` column's read-path tests (`an_old_shape_row_with_no_kind_recovers_exactly_as_before`, `recovery_reconstructs_the_same_edit_kind_sequence_the_live_session_pushed`); split candidate: move the `#[cfg(test)] mod tests` block to a sibling `snapshot_tests.rs`, the `rune-db/src/load.rs`/`load_tests.rs` pattern)
  - `crates/rune-db/tests/multiprocess/helper.rs` — 508 (newly over — `EditBatch` bundling for `Store::append_edit`'s clippy-argument-count fix widened every call site; test file)
- **Wrong**: source files exceed the 500-line house rule, none ledgered. This pass (moving nine in-file `#[cfg(test)] mod tests` blocks to `#[path]`-included siblings, the `rune-db/src/load.rs`/`load_tests.rs` pattern) dropped ten files below 500 and off this list: `crates/rune-merge/src/hunks.rs` (490), `crates/rune-tui/src/guard.rs` (452), `crates/rune-vfs/src/publish.rs` (225), `crates/rune-db/src/sync.rs` (207, but its extracted `sync_tests.rs` sibling is itself over — see above), `crates/rune-db/src/probe.rs` (160), `crates/rune-tui/src/merge/landing.rs` (411), `crates/rune-tui/src/footer_hints.rs` (247), `crates/rune-tui/src/commands/mouse.rs` (337). Unrelated ordinary growth independently dropped `crates/rune-tui/src/footer.rs`, `crates/rune-fuzz/src/driver/mod.rs`, and `crates/rune-tui/src/focus.rs` below 500 since the last measurement; `crates/rune-db/src/writer.rs` also dropped below 500 (its now-moot split candidate, moving `execute_op`'s match into a sibling `writer_exec.rs`, is dropped). Five files dropped below 500 in an earlier pass and stay off this list: `crates/rune-tui/src/materialize_ack.rs` (305), `crates/rune-tui/src/materialize_ack/reactions.rs` (378), `crates/rune-md/src/emit/mod.rs` (343), `crates/rune-syntax/src/wrap/mod.rs` (494). `crates/rune-syntax/src/syntax.rs` (466) and `crates/rune-tui/src/save/materialize.rs` (329) remain under the threshold from an earlier drop.
- **Instead**: split each per its own named candidate, once identified; comment purge (next entry) likely shrinks several below the threshold on its own.
- **Done when**: this list is empty (files legitimately re-measured after the comment purge, then split as needed).

### Comment purge (the refactor itself)
- **Where**: `crates/rune-tui` broadly — comments are roughly a third of the crate, rustdoc included
- **Wrong**: a paragraph-long justification comment indicts the code it justifies — the code is the refactor candidate, not the comment.
- **Instead**: apply the heuristic crate-wide: keep only complex-algorithm explanations (inside the function), third-party quirks that save real debugging time, and constraints no type/name/test can carry; delete the rest by cleaning the code they were defending.
- **Done when**: the purge has run and each surviving comment matches one of the three legitimate categories above.


## Mutation testing (2026-08-27)

cargo-mutants is integrated: config in `.cargo/mutants.toml`, `make mutants`
(`PKG=<crate>` scopes, `J=` jobs, `MUTANTS_ARGS=` passthrough). Seven crates
were driven to zero unjustified missed mutants; every exclusion in
`.cargo/mutants.toml` carries its proof in the commit that added it.

### Covered (missed mutants at pass end / total generated)
- rune-image 0/143 · rune-nav 0/56 · rune-syntax 0/397 · rune-core 0/309 ·
  rune-ts 0/103 · rune-cli 1/99 · rune-md 2/869 (final verify pass pending
  at ledger time; residuals below)

### Known residual misses (documented, deliberately not excluded)
- `crates/rune-cli/src/main.rs` `AppGuard::drop -> ()` — proving the drop's
  effect needs an interactive session or a sleep-free sync point that does
  not exist; a future PTY harness can claim it.
- `crates/rune-md/src/invariant.rs` `assert_no_duplicate_content{,_at} -> ()`
  — killing them needs a document that violates the invariant; 100k+ fuzz
  inputs found none because the pipeline appears correct. A regression should
  still be able to surface them.

### Not yet mutation-tested
- rune-merge, rune-vfs, rune-db — deferred: they carried in-flight DATA-LOSS
  fixes from the 2026-08-26 sweep during this pass; run them once that work
  settles. rune-vfs/rune-db are the prime-directive crates — highest value
  next.
- rune-tui (51k LOC) and rune-fuzz (the harness itself) — out of scope this
  pass by size and role.

### Caveats and follow-ups
- This pass ran on Linux. `#[cfg(target_os = "macos")]` blocks — including
  the exchange/fsync branches in `crates/rune-vfs/src/disk.rs` and
  `crates/rune-db/src/session.rs` — were never compiled, so no mutants were
  generated for them. A macOS run is needed before "0 missed" means anything
  for the durability paths.
- Some `exclude_re` entries pin `file:line` (noted in their commits); they
  need re-justification when those lines move.
- Cheap CI follow-up: a PR-scoped `cargo mutants --in-diff` job (the book's
  pr-diff workflow) once a macOS runner budget exists.
- The `--skip` test-name filters matter: `dependency_guard` (rune-nav) and
  the source-grep gate `self_state_assignment_is_scoped_to_the_two_transition_writers`
  (rune-md) read the tree, not the mutant, and must not run under mutants —
  the rune-md gate false-catches mutants if left in.
