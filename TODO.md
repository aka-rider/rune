# TODO — Refactor Ledger

Reinstated 2026-08-07 from a three-audit sweep of the codebase. This is an
inventory of known violations of `CONSTITUTION.md`'s governing rules and of
non-idiomatic implementations with better alternatives available; it seeds a
future refactor plan. New code must not repeat these patterns even where
legacy code still does. Fixes land with tests per the constitution, and an
entry is deleted in the same commit that fixes it.

## Data-safety

### rune-merge re-anchors conflict sections by content search, not position accounting
- **Where**: `crates/rune-merge/src/hunks.rs` (`anchor_section`, `find_subslice`)
- **Wrong**: `anchor_section` re-anchors each diff3 section by first-occurrence substring search in `ours`/`theirs`; a section whose bytes repeat earlier in the input can anchor at the wrong occurrence — silent, on a data-destructive path. (The marker-collision half of this entry is fixed: marker length is now scanned from the inputs and always exceeds any marker-shaped document line, so segmentation collision is unrepresentable; a failed anchor already widens only its own conflict run.)
- **Instead**: anchor by consumed-byte accounting, or obtain structured hunks without parsing rendered text.
- **Done when**: anchoring is position-accounted rather than content-searched.

## Architecture

### TABLE-ROW-WIDTH lives twice, at two genuinely different layers
- **Where**: `crates/rune-md/tests/table_row_width_lone_cr.rs` (`assert_every_table_group_has_uniform_width`, over `&[SyntaxLine]`/source text — rune-syntax's own pre-wrap table geometry); `crates/rune-fuzz/src/invariant/render.rs` (`table_row_width`, over `Snapshot.cells`/`row_meta.table_group` — rune-tui's post-wrap, post-box-drawing rendered cells)
- **Wrong**: assigned as a dedup ("delete the hand-port, call the fuzz checker"), but investigation shows these are not the same computation at two call sites — they check different data at different pipeline stages. The rune-md test sums display width of each source `SyntaxLine`'s own span text, grouped by contiguous `.table.is_some()` runs, before any wrap/box synthesis. The fuzz checker sums `Cell.width` over post-viewport, post-`expand_tables` rendered rows, filtered to `RowMeta.boxed` (deliberately excluding the ragged Pivoted layout, which the rune-md-level check doesn't and can't distinguish since `boxed` isn't visible on `SyntaxLine`). rune-md cannot depend on rune-fuzz/rune-tui at all (rune-fuzz depends on rune-md; only a dev-dependency cycle is possible, and even that would require driving the whole `App`/render pipeline from an rune-md regression test whose whole point is a parse/line-index bug, not rendering). This mirrors the codebase's own established precedent for this exact situation (`crates/rune-fuzz/src/invariant/wrap.rs`'s `wrap_line_lens` doc comment, cross-referencing `rune-md/tests/wrap_roundtrip.rs`'s `syntax_line_byte_len` by name instead of calling it, for the identical dependency-direction reason).
- **Instead**: either accept these as two independently-necessary checks (and drop the "hand-port" framing), or, if the SyntaxLine-level check should be the SOLE source of truth, move it down into `rune-syntax`/`rune-md` production code as a real invariant (`assert_invariant!` in `emit`) and have the fuzz checker's `table_row_width` become a thinner secondary check layered on top of what production already guarantees — but that changes what TABLE-ROW-WIDTH catches (a table box malformed only by the wrap/box-drawing pass, downstream of `SyntaxLine`, would no longer be independently caught).
- **Done when**: someone with the full TABLE-ROW-WIDTH history decides which of the two outcomes above is correct; until then both checks stay as they are.

### Session-driver migration residue
- **Where**: `crates/rune-tui/tests/rename_common/mod.rs` (kept App-layer fixtures: `seeded_vfs`, `app_with`, `app_with_store`, `unsaved_named_app_with_store`, `next_event`, the `wait_for_*` waits, `send`, `type_text`, `type_new_name`); its consumers are THIRTEEN test binaries, not six: `bind_new_named.rs`, `save_state_machine.rs`, `materialize_dead_writer_reentrancy.rs`, `materialize_fatal_two_docs.rs`, `refused_hydration_detach.rs`, `reading_view.rs`, `rename_gate.rs`, `rename_bind.rs`, `rename_refusals.rs`, `rename_collision.rs`, `rename_replace.rs`, `rename_clipboard.rs`, `rename_focus.rs`; `navhistory_common` (embeds `explorer_common` via `#[path]`, still builds bare `App`s) with consumers `navhistory.rs`/`navhistory_browse.rs`; `set_doc_db_for_test` (consumed by the kept fixtures, `materialize_fatal_two_docs.rs`, and `g7_shared_file_baseline.rs`).
- **Wrong**: the 2026-08-13 migration moved nine `*_common` modules onto `rune_fuzz::Session`, but these binaries still construct bare `App`s through the duplicated fixture layer the migration exists to delete, so `rename_common` carries both layers side by side.
- **Instead**: migrate the six binaries and `navhistory_common` onto `Session`, then delete the App layer and re-evaluate whether `set_doc_db_for_test` is orphaned. Known driver gaps that blocked full migration, to close in `rune-fuzz` first: no out-of-order db-op delivery through checked steps (`deliver_db` is oldest-first; `merge_common::deliver_op_unchecked` is the workaround); the redivergence tracker only learns of external writes via `Action::DivergeDisk`; `Effects::raw`/timer arming invisible through `Session`; `ReadDir`/`ReadFile` Cmds dropped by the driver; a single rename-Cmd slot; no targeted `ClipboardRead` action; `SAVE-INFLIGHT-SM` rejects the legitimate `bind_new_now` Enter-materialize flip.
- **Done when**: no test binary constructs an `App` through a `*_common` fixture that duplicates `Session`, and the driver gaps above are either closed or individually recorded as deliberate.
- **Where**: `is_dirty_cached` (`document/mod.rs`) read by `Document::dirty_for_render`/`App::dirty_for_render` vs the re-derive at `materialize_ack::is_dirty_now`
- **Investigated and rejected**: the split itself is deliberate — a decision (close/quit/save/trash gating) must re-derive dirty fresh, while render (footer, title, tab markers, hints) can read a cache that two chokepoints (`materialize_ack::recompute_dirty`'s edit/ack call sites, and `is_dirty_now` itself) keep current between transitions. The hazard was never the split; it was the neutral name `is_dirty`/`is_dirty()` inviting a decision site to reach for the cache. Both `Document`'s and `App`'s cache accessors are now named `dirty_for_render`, so a decision site reaching for `is_dirty_now` instead is the only spelling that reads as a decision at all. Every production decision site (`save.rs`, `pane.rs`, `trash.rs`, `workspace/close.rs`, `opentabs/limit.rs`) already calls `is_dirty_now`; `guard.rs`'s and `rune-cli/open.rs`'s own call sites are test assertions checking the cache's own freshness after an operation, not decisions, and stay on `dirty_for_render`.
- **Update**: the `save_in_flight` half of this entry is resolved — `Document.save_in_flight` is now a derived accessor over the `SaveState` machine (`document/save_state.rs`), and no test can manufacture an impossible in-flight state by writing a field directly.

### Sentinel-value residue
- **Where**: `crates/rune-tui/src/app.rs` (`frame_height/width: u16`, 0 = no resize yet — its own entry below defers it); `crates/rune-tui/src/filesearch/rank.rs` and `crates/rune-tui/src/messages/mod.rs` (`unwrap_or(usize::MAX)` — both documented deliberate orderings); `rune-nav`'s `resolve` still takes a bare `&Path`, so an unresolved root crosses that boundary as an empty `PathBuf` via `unwrap_or_default()`.
- **Wrong**: the class is otherwise closed (`CellMap`, table `buf`, `Cell.buf_offset` are `Option<u32>`; `App.root` is `Option<PathBuf>`); these are the remaining sites where an absent value borrows a valid-looking encoding.
- **Instead**: `Option<&Path>` through `rune-nav::resolve` if the family sweep continues; the two `usize::MAX` orderings stay unless their modules change anyway.
- **Done when**: `rune-nav` no longer receives an empty path for "no root", or the entry records why the boundary deliberately stays.

### Unbounded thread-per-Cmd in `spawn_cmd`
- **Where**: `crates/rune-tui/src/runtime/mod.rs` (`spawn_cmd`)
- **Wrong**: `spawn_cmd` spawns one OS thread per `Cmd` with no pool bound; `Highlight`/`ImageDecode` issue at keystroke rate. (The sleep-based confirm/collapse timeouts are gone — all deadlines route through the keyed `TimerService`.)
- **Instead**: bound the worker pool.
- **Done when**: `spawn_cmd` is bounded.

### A generation counter's type doesn't say which feature it belongs to
- **Where**: `crates/rune-tui/src/generation.rs`'s `Generation`/`GenCounter` — one shared type now minted by every counter (`next_rename_gen`, `next_merge_gen`, `next_save_confirm_gen`, `next_quit_gen`, `next_trash_gen`, `next_filesearch_gen`, `next_search_history_gen`, `Explorer::next_request_gen`, `MessageLog::generation`, `ImageState::next_generation`); each is compared against a bare `generation: Generation` field carried by its own `Msg` reply.
- **Wrong**: every counter and every `Msg` field that answers it share the exact same `Generation` type. Nothing stops a future edit from comparing one feature's counter against another feature's reply — reading `next_quit_gen` where `next_rename_gen` belongs, say — and the mistake still compiles.
- **Instead**: give `Generation`/`GenCounter` a phantom type parameter naming the feature it belongs to (`Generation<Rename>`, `Generation<Merge>`, and so on), so passing a rename generation where a merge generation is expected fails to compile instead of comparing two unrelated counters at runtime.
- **Done when**: each feature's reply carries a `Generation<T>` typed to that feature, and swapping two features' generation values is a compile error rather than a stale-reply check that only catches it at runtime.
- **Update**: wide churn — every counter and every `Msg` reply site would need the change — and the failure mode a mismatch causes today (a stale reply gets discarded as "not the generation we're waiting for") already fails safe, so this is recorded rather than fixed now. Nine of the counters listed above now mint through the shared `Generation`/`GenCounter` newtype (`crates/rune-tui/src/generation.rs`), closing the prior mint-order inconsistency and per-feature reimplementation — but not this entry's actual complaint, which is the shared type itself.

### Frame size `0` doubles as "not yet measured"
- **Where**: `crates/rune-tui/src/app.rs`'s `App::frame_width`/`frame_height`, both `u16`; guarded by an early return in `crates/rune-tui/src/focus.rs` (`if app.frame_width == 0 || app.frame_height == 0`) and read again at other layout call sites.
- **Wrong**: `0` means both "the first resize hasn't landed yet" and, in principle, a real if degenerate frame size. The two fields are read independently at some call sites, so a caller can observe one field measured and the other still `0` with nothing in the type system marking that in-between state.
- **Instead**: replace the pair with a single `Option<(u16, u16)>` (or a small named struct) that is `None` until the first resize lands, so "not measured yet" is a state the type carries instead of a value borrowed from the field's own valid range.
- **Done when**: `frame_width`/`frame_height` no longer use `0` as a sentinel.
- **Update**: today's guard is one check in `focus.rs` plus the layout paths that read both fields together — low value for the size of the change, so this is recorded rather than fixed now.

### `Cursor::desired_col` mixes a Syntax-Space column with Buffer-Space byte offsets
- **Where**: `crates/rune-core/src/cursor.rs`'s `Cursor` struct — `position` and `anchor` are byte offsets in Buffer Space, and `desired_col`, declared right next to them, is a column in Syntax Space (the column layout produces after wrapping and concealment); mirrored in the persisted schema at `crates/rune-db/src/payload.rs`'s `CursorPayload`, which stores all three as bare `usize` fields.
- **Wrong**: no `ByteOffset` or `SyntaxCol` newtype exists anywhere in the crate, so `position`, `anchor`, and `desired_col` are all just `usize`. A future edit that threads a byte offset through code expecting a Syntax-Space column, or the reverse, compiles without complaint.
- **Instead**: introduce typed wrappers for the two coordinate spaces — a `ByteOffset` for `position`/`anchor`, a `SyntaxCol` or similar for `desired_col` — so mixing them becomes a compile error. Do this together with the next change to `crates/rune-db/src/payload.rs`'s cursor schema, since typing `desired_col` crosses the on-disk cursor payload and any schema change already forces a review of that boundary.
- **Done when**: `Cursor`'s three fields have distinct types for their two coordinate spaces, and `CursorPayload`'s fields mirror that typing (or the entry records why the persisted form deliberately stays untyped).

## Mechanical

### O(file) per keystroke is the deliberate design ceiling
- **Where**: `crates/rune-core/src/buffer/mod.rs`, `crates/rune-core/src/buffer/lineindex.rs`, `crates/rune-tui/src/commands/edit_core.rs`, `crates/rune-tui/src/materialize_ack.rs`; perf-guarded by `crates/rune-tui/tests/perf_guard.rs:92` (`keystroke_view_cost_under_budget_on_a_5k_line_code_document`)
- **Wrong**: full content copy + full line-index clone + full memcmp + journal clones per edit batch; does not scale past the guard fixture's size.
- **Instead**: a rope with the same value-semantics facade, if the ceiling is ever hit in practice.
- **Done when**: not currently actionable — record only; revisit if the perf guard's fixture size stops matching real documents.

### Files over 500 lines
- **Where** (recomputed from the live tree with `wc -l`; comment purge below will change these numbers):
  - `crates/rune-cli/src/bootstrap_tests.rs` — 1119 (test file; split candidate unchanged: move the launch-image-first tests (`launch_image_first_*`) plus `CountingReadVfs` to a sibling `bootstrap_tests_image.rs`, `#[path]`-included from `main.rs` the way `rune-db`'s `load_tests.rs` is from `load.rs`)
  - `crates/rune-tui/src/explorer_preview/tests.rs` — 1083 (test file)
  - `crates/rune-tui/src/global.rs` — 886 (grew from 793: WP6 `DIFF_BINDINGS` registration into `claimants_across_pane_tables` plus the new bidirectional collision guard test)
  - `crates/rune-tui/tests/diff_view.rs` — 801 (test file; newly over — the diff-view plan's own test suite grew through WP3-WP8: layout/alignment/intraline tests plus WP6's verb/chord/click tests all landed here; split candidate: move the verb and chord tests, `take_theirs_makes_the_region_same_and_undoes_in_one_step` through `click_in_the_left_pane_moves_the_right_pane_caret_to_the_aligned_line`, to a sibling `diff_view_verbs.rs`, leaving the layout/alignment/intraline tests here)
  - `crates/rune-vfs/src/mem.rs` — 766 (`fail_resolve` and its tests pushed this further over)
  - `crates/rune-tui/src/pane.rs` — 761 (grew from 862 previously, now settled here: the `NavBack`/`NavForward` dispatch arms plus later routing growth)
  - `crates/rune-tui/src/runtime/mod.rs` — 706 (grew from 621)
  - `crates/rune-fuzz/src/generate/palette.rs` — 702 (grew from 659: the `NAV_BACK_KEY`/`NAV_FORWARD_KEY` consts, plan WP8)
  - `crates/rune-db/src/schema.rs` — 684 (newly over — named-draft session support (`3d7af356`) grew the schema DDL)
  - `crates/rune-tui/src/document/mod.rs` — 679 (split candidate unchanged: move the `ReadOnly` enum plus its `impl` block, which don't depend on `Document`'s own fields, to a sibling `read_only.rs`)
  - `crates/rune-tui/src/db.rs` — 668 (split candidate unchanged: move the `FileBinding`/`DocDb` type definitions to a sibling `db_types.rs`, keeping the `Db`/writer-bridge wiring here)
  - `crates/rune-tui/src/linemap.rs` — 663 (newly over — the typed-offsets/image-id rework (`8cdaaef3`) grew this)
  - `crates/rune-tui/src/app.rs` — 619 (grew from 602)
  - `crates/rune-db/tests/multiprocess/scenarios.rs` — 617 (test file)
  - `crates/rune-tui/tests/rename_focus.rs` — 613 (test file)
  - `crates/rune-fuzz/src/script/mod.rs` — 611 (newly over — mouse-event action support (`b970c59c`) grew the script table)
  - `crates/rune-fuzz/src/driver/session.rs` — 592 (already over at 533 before a boot()-splitting cleanup added the named `new_app`/`open_seed_document`/`live_session`/`panicked_session` substeps; split candidate: move `Session::boot` and its four helpers, plus the `Seed`/`new_state` plumbing they share, to a sibling `boot.rs`, leaving `Session`'s post-boot methods here)
  - `crates/rune-db/src/sync_tests.rs` — 592 (newly over — this is the sibling test module split out of `sync.rs` in this pass; the moved test block was already this size in-file, so it needs its own further split, no candidate identified yet)
  - `crates/rune-tui/src/filesearch/tests.rs` — 591 (test file)
  - `crates/rune-fuzz/src/generate/cluster.rs` — 585 (grew from 505: plan WP8's `cluster_caret_history`/`cluster_advance_clock` plus WP9's merge-chord rework of `cluster_merge`'s own doc comment; split candidate: move the merge/highlight/multicursor cluster functions, `cluster_merge` through `cluster_multicursor`, to a sibling `cluster_scenarios.rs`, leaving the simpler single-shape clusters and `arb_cluster` itself here)
  - `crates/rune-tui/src/workspace/mod.rs` — 567 (pushed over by the `resolve_or_report` chokepoint added alongside the `resolve` signature change; split candidate: move the `#[cfg(test)] mod tests` block to a sibling test module)
  - `crates/rune-tui/src/render/filesearch.rs` — 559
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
  - `crates/rune-tui/src/dispatch.rs` — 517 (grew from 536, settled lower after fmt reflow but still over)
  - `crates/rune-tui/tests/tui_edit.rs` — 516 (newly over — vertical-motion caret-resettle tests (`c13d70fc`) grew this)
  - `crates/rune-db/src/adopt.rs` — 516 (newly over — abandoned-resolve retention logic (`d8f69e9a`) grew this)
  - `crates/rune-tui/src/commands/edit.rs` — 514 (newly over — the selection-consumption fix (`d1543de5`) grew this)
  - `crates/rune-tui/src/field.rs` — 512 (newly over — the typed-offsets rework (`8cdaaef3`) grew this)
  - `crates/rune-md/src/catalogue.rs` — 512
  - `crates/rune-tui/src/render/overlay.rs` — 511 (newly over — the Option-typed cell-offset rework (`008b9adf`) grew this)
  - `crates/rune-md/src/parse/inline.rs` — 511 (newly over — the forward-scan code-span fix (`78abf7a0`) grew this)
  - `crates/rune-fuzz/src/script/decode.rs` — 509 (over again — previously dropped to 413, mouse-action decode support (`b970c59c`) grew it back over)
- **Wrong**: source files exceed the 500-line house rule, none ledgered. This pass (moving nine in-file `#[cfg(test)] mod tests` blocks to `#[path]`-included siblings, the `rune-db/src/load.rs`/`load_tests.rs` pattern) dropped ten files below 500 and off this list: `crates/rune-merge/src/hunks.rs` (490), `crates/rune-tui/src/guard.rs` (452), `crates/rune-vfs/src/publish.rs` (225), `crates/rune-tui/src/db_ack.rs` (429), `crates/rune-db/src/sync.rs` (207, but its extracted `sync_tests.rs` sibling is itself over — see above), `crates/rune-db/src/probe.rs` (160), `crates/rune-tui/src/merge/landing.rs` (411), `crates/rune-tui/src/footer_hints.rs` (247), `crates/rune-tui/src/commands/mouse.rs` (337). Unrelated ordinary growth independently dropped `crates/rune-tui/src/footer.rs`, `crates/rune-fuzz/src/driver/mod.rs`, and `crates/rune-tui/src/focus.rs` below 500 since the last measurement; `crates/rune-db/src/writer.rs` also dropped below 500 (its now-moot split candidate, moving `execute_op`'s match into a sibling `writer_exec.rs`, is dropped). Five files dropped below 500 in an earlier pass and stay off this list: `crates/rune-tui/src/materialize_ack.rs` (305), `crates/rune-tui/src/materialize_ack/reactions.rs` (378), `crates/rune-md/src/emit/mod.rs` (343), `crates/rune-syntax/src/wrap/mod.rs` (494). `crates/rune-syntax/src/syntax.rs` (466) and `crates/rune-tui/src/save/materialize.rs` (329) remain under the threshold from an earlier drop.
- **Instead**: split each per its own named candidate, once identified; comment purge (next entry) likely shrinks several below the threshold on its own.
- **Done when**: this list is empty (files legitimately re-measured after the comment purge, then split as needed).

### Comment purge (the refactor itself)
- **Where**: `crates/rune-tui` broadly — comments are roughly a third of the crate, rustdoc included
- **Wrong**: a paragraph-long justification comment indicts the code it justifies — the code is the refactor candidate, not the comment.
- **Instead**: apply the heuristic crate-wide: keep only complex-algorithm explanations (inside the function), third-party quirks that save real debugging time, and constraints no type/name/test can carry; delete the rest by cleaning the code they were defending.
- **Done when**: the purge has run and each surviving comment matches one of the three legitimate categories above.

