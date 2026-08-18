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
- **Where**: `crates/rune-tui/tests/rename_common/mod.rs` (kept App-layer fixtures: `seeded_vfs`, `app_with`, `app_with_store`, `unsaved_named_app_with_store`, `next_event`, the `wait_for_*` waits, `send`, `type_text`, `type_new_name`); their remaining consumers `bind_new_named.rs`, `save_state_machine.rs`, `materialize_dead_writer_reentrancy.rs`, `materialize_fatal_two_docs.rs`, `refused_hydration_detach.rs`, `reading_view.rs`; `navhistory_common` (embeds `explorer_common` via `#[path]`, still builds bare `App`s); `set_doc_db_for_test` (still consumed by the kept fixtures and `g7_shared_file_baseline.rs`).
- **Wrong**: the 2026-08-13 migration moved nine `*_common` modules onto `rune_fuzz::Session`, but these binaries still construct bare `App`s through the duplicated fixture layer the migration exists to delete, so `rename_common` carries both layers side by side.
- **Instead**: migrate the six binaries and `navhistory_common` onto `Session`, then delete the App layer and re-evaluate whether `set_doc_db_for_test` is orphaned. Known driver gaps that blocked full migration, to close in `rune-fuzz` first: no out-of-order db-op delivery through checked steps (`deliver_db` is oldest-first; `merge_common::deliver_op_unchecked` is the workaround); the redivergence tracker only learns of external writes via `Action::DivergeDisk`; `Effects::raw`/timer arming invisible through `Session`; `ReadDir`/`ReadFile` Cmds dropped by the driver; a single rename-Cmd slot; no targeted `ClipboardRead` action; `SAVE-INFLIGHT-SM` rejects the legitimate `bind_new_now` Enter-materialize flip.
- **Done when**: no test binary constructs an `App` through a `*_common` fixture that duplicates `Session`, and the driver gaps above are either closed or individually recorded as deliberate.
- **Where**: `is_dirty_cached` (`document/mod.rs:142`) vs `is_dirty` (`document/mod.rs:360-361`) vs `is_dirty_now` (`crates/rune-tui/src/materialize_ack.rs:408`)
- **Wrong**: two accessors exist where picking the wrong one is a per-call-site correctness decision, for a compare the code's own comment calls "length check + memcmp, microsecond-scale" — the cache buys only a staleness hazard.
- **Instead**: delete the dirty cache or store a content hash instead.
- **Done when**: one of the two is deleted.
- **Update**: the `save_in_flight` half of this entry is resolved — `Document.save_in_flight` is now a derived accessor over the `SaveState` machine (`document/save_state.rs`), and no test can manufacture an impossible in-flight state by writing a field directly.

### Sentinel-value residue
- **Where**: `crates/rune-tui/src/app.rs` (`frame_height/width: u16`, 0 = no resize yet — its own entry below defers it); `crates/rune-tui/src/filesearch/rank.rs` and `crates/rune-tui/src/messages/mod.rs` (`unwrap_or(usize::MAX)` — both documented deliberate orderings); `rune-nav`'s `resolve` still takes a bare `&Path`, so an unresolved root crosses that boundary as an empty `PathBuf` via `unwrap_or_default()`.
- **Wrong**: the class is otherwise closed (`CellMap`, table `buf`, `Cell.buf_offset` are `Option<u32>`; `App.root` is `Option<PathBuf>`); these are the remaining sites where an absent value borrows a valid-looking encoding.
- **Instead**: `Option<&Path>` through `rune-nav::resolve` if the family sweep continues; the two `usize::MAX` orderings stay unless their modules change anyway.
- **Done when**: `rune-nav` no longer receives an empty path for "no root", or the entry records why the boundary deliberately stays.

### Nine hand-rolled generation counters
- **Where**: `crates/rune-tui/src/app.rs:120,127,178,195,237,267,288` (`next_rename_gen`, `next_merge_gen`, `next_save_confirm_gen`, `next_quit_gen`, `trash_gen`, `next_filesearch_gen`, `next_search_history_gen`) plus `explorer.request_generation`, `messages.generation`, `ImageState::next_generation`; inconsistent mint order between `crates/rune-tui/src/filesearch/mod.rs:108-109` (mint-then-read) and `crates/rune-tui/src/pane.rs:363-364` (read-then-mint)
- **Wrong**: nine bespoke counters, each with its own mint site and stale-discard check, mint-then-read inconsistent with read-then-mint between sites.
- **Instead**: one `Gen<T>` newtype (`mint`/`is_current`), type-distinct per domain.
- **Done when**: all generation fields route through one newtype with one mint convention.

### Sleep-based uncancellable timers; unbounded thread-per-Cmd
- **Where**: `crates/rune-tui/src/save.rs:215`, `crates/rune-tui/src/pane.rs:400`, `crates/rune-tui/src/messages/mod.rs:451` (`thread::sleep`-based `Cmd` timeouts); correct shape at `crates/rune-tui/src/runtime/snapshot_timer.rs`; unbounded spawn at `crates/rune-tui/src/runtime/mod.rs:469` (`spawn_cmd`)
- **Wrong**: confirm/collapse timeouts park one OS thread per (re)arm with no cancellation — the generation counters exist largely to discard the late replies. `spawn_cmd` spawns unbounded threads; `Highlight`/`ImageDecode` issue at keystroke rate.
- **Instead**: generalize `SnapshotTimer` (single thread, Mutex+Condvar, rearm-to-earliest) to a keyed deadline map; bound the worker pool.
- **Done when**: no `Cmd` does a bare `thread::sleep`, and `spawn_cmd` is bounded.

### A generation counter's type doesn't say which feature it belongs to
- **Where**: `crates/rune-tui/src/app.rs`'s `next_rename_gen: u32`, `next_merge_gen: u32`, `next_save_confirm_gen: u32`, `next_quit_gen: u32`, `trash_gen: u32`, `next_filesearch_gen: u64`, `next_search_history_gen: u64`; each is compared against a bare `generation: u32`/`generation: u64` field carried by its own `Msg` reply, for example in `crates/rune-tui/src/rename.rs`, `crates/rune-tui/src/merge/state.rs`, `crates/rune-tui/src/trash.rs`, and `crates/rune-tui/src/filesearch/mod.rs`.
- **Wrong**: every counter and every `Msg` field that answers it share the same primitive type. Nothing stops a future edit from comparing one feature's counter against another feature's reply — reading `next_quit_gen` where `next_rename_gen` belongs, say — and the mistake still compiles. The "Nine hand-rolled generation counters" entry above already proposes consolidating these into one `Gen<T>` newtype; this entry names the specific hazard that consolidation must close, or it survives the merge: `T` has to be a type parameter distinct per feature (`Gen<Rename>`, `Gen<Merge>`, and so on), not just one shared `Gen` wrapping a plain integer.
- **Instead**: give the consolidated `Gen<T>` a phantom type parameter naming the feature it belongs to, so passing a rename generation where a merge generation is expected fails to compile instead of comparing two unrelated counters at runtime.
- **Done when**: each feature's reply carries a `Gen<T>` typed to that feature, and swapping two features' generation values is a compile error rather than a stale-reply check that only catches it at runtime.
- **Update**: wide churn — every counter and every `Msg` reply site would need the change — and the failure mode a mismatch causes today (a stale reply gets discarded as "not the generation we're waiting for") already fails safe, so this is recorded rather than fixed now.

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

### `bind_new: bool` stands in for a two-way publish-mode choice
- **Where**: `crates/rune-tui/src/db.rs`'s `DocDb::bind_new` field; read in `crates/rune-tui/src/save/materialize.rs` (`materialize_now`/`bind_new_now`) to choose between `PutCondition::IfAbsent` and `PutCondition::Force`; checked again in `crates/rune-tui/src/materialize_ack/reactions.rs`'s `lost_create_race` and `naming_collision`; flipped back to `false` on a successful publish in `crates/rune-tui/src/materialize_ack/committed.rs`'s `handle_committed_ack`.
- **Wrong**: a `bool` reads as an on/off switch, but `bind_new` actually selects between two distinct publish modes — create-only versus overwrite-an-established-target — that ripple through several unrelated call sites. Each reader has to already know that `true` means "no CAS baseline exists yet, publish must not clobber a concurrent creator" and `false` means "an established baseline exists, publish as an ordinary overwrite"; that meaning lives in comments and call-site knowledge, not in the type.
- **Instead**: replace the field with a two-variant publish-mode enum naming the create-only and overwrite-existing cases, so every branch that reads it states the mode explicitly instead of decoding a bare `true`/`false`.
- **Done when**: `DocDb` no longer carries a bare `bind_new: bool`; every site listed above matches on a named publish-mode enum instead.
- **Update**: `rune-db`'s own equivalent flag was already promoted to the `MaterializeTarget` enum (`crates/rune-db/src/materialize_types.rs`); `rune-tui`'s `bind_new` was deliberately left as a `bool` when that landed, and this entry is the deferred follow-up on the `rune-tui` side.

## Mechanical

### Typed errors flattened to String
- **Where**: ~9 `map_err(|e| e.to_string())` at Cmd boundaries across `runtime/mod.rs`, `save.rs`, `trash.rs`, `rename_create.rs`, `graphics/*`; inside `rune-db::Error` (`crates/rune-db/src/error.rs:17,37,49,60`): `ReplayFailed(String)`, `CorruptPayload(String)`, `SessionEstablish(String)` stringify their sources while `Sqlite(rusqlite::Error)` proves the crate can hold typed sources
- **Wrong**: stringifying erases the `ErrorKind`/error type that `rune-vfs::WrappedIo` and `rusqlite::Error` deliberately preserve.
- **Instead**: typed variants; a small `Cause` enum in `Msg::Error`.
- **Done when**: no Cmd-boundary error is stringified before it reaches its handler, and `rune-db::Error`'s String variants hold typed sources.

### Stale/false comments (provable lies)
- **Where**: "nightly-only" claims (see the `char_boundary` entry above); `crates/rune-tui/tests/db_wiring_hydrate.rs:4` (cites a deleted per-crate `TODO.md`); `crates/rune-syntax/src/wrap/width.rs:93`, `crates/rune-tui/tests/tui_render_text.rs:387` (cite a nonexistent `TODO/TODO.md`)
- **Wrong**: comments cite functions and files that no longer exist.
- **Instead**: fix or delete each citation when touched (per house rule, no `path:line` in comments either).
- **Done when**: no comment in the tree cites a nonexistent symbol or deleted file.

### O(file) per keystroke is the deliberate design ceiling
- **Where**: `crates/rune-core/src/buffer/mod.rs`, `crates/rune-core/src/buffer/lineindex.rs`, `crates/rune-tui/src/commands/edit_core.rs`, `crates/rune-tui/src/materialize_ack.rs`; perf-guarded by `crates/rune-tui/tests/perf_guard.rs:92` (`keystroke_view_cost_under_budget_on_a_5k_line_code_document`)
- **Wrong**: full content copy + full line-index clone + full memcmp + journal clones per edit batch; does not scale past the guard fixture's size.
- **Instead**: a rope with the same value-semantics facade, if the ceiling is ever hit in practice.
- **Done when**: not currently actionable — record only; revisit if the perf guard's fixture size stops matching real documents.

### Files over 500 lines
- **Where** (recomputed from the live tree with `wc -l`; comment purge below will change these numbers):
  - `crates/rune-db/src/sync.rs` — 801 (split candidate: move the `#[cfg(test)]` module to a sibling `sync_tests.rs`, the `materialize.rs`/`materialize_tests.rs` pattern this crate already uses)
  - `crates/rune-tui/src/explorer_preview/tests.rs` — 1064 (test file)
  - `crates/rune-tui/src/global.rs` — 848 (grew from 793: WP6 `DIFF_BINDINGS` registration into `claimants_across_pane_tables` plus the new bidirectional collision guard test)
  - `crates/rune-tui/src/pane.rs` — 866 (grew from 862: the `NavBack`/`NavForward` dispatch arms)
  - `crates/rune-merge/src/hunks.rs` — 702 (the `#[cfg(test)] mod tests` block is over half the file — split candidate: move it to a `#[path]`-included sibling test module so it keeps access to the private `parse_hunks`/`anchor_section` it exercises)
  - `crates/rune-tui/src/runtime/mod.rs` — 649 (grew from 621)
  - `crates/rune-fuzz/src/generate/palette.rs` — 686 (grew from 659: the `NAV_BACK_KEY`/`NAV_FORWARD_KEY` consts, plan WP8)
  - `crates/rune-tui/src/app.rs` — 605 (grew from 602)
  - `crates/rune-tui/tests/rename_focus.rs` — 606 (test file)
  - `crates/rune-tui/src/filesearch/tests.rs` — 586 (test file)
  - `crates/rune-tui/src/merge/landing.rs` — 701 (grew again with WP7's pane install: `build_pane_install`/`auto_applied_entries` replaced the marker builder in-file; split candidate unchanged: move the `#[cfg(test)] mod tests` block, over a third of the file, to `crates/rune-tui/tests/merge_landing_unit.rs` or keep it `#[path]`-included from `landing.rs` if it needs the private fns it exercises)
  - `crates/rune-tui/src/db.rs` — 552 (split candidate unchanged: move the `FileBinding`/`DocDb` type definitions to a sibling `db_types.rs`, keeping the `Db`/writer-bridge wiring here)
  - `crates/rune-tui/src/db_ack.rs` — 697 (the binding/replica-seam work — `Replica::take_pending`'s call sites, the hardlink-fork load warning and its tests — has pushed this further over collectively, no single change owning it; split candidate unchanged: move the `#[cfg(test)] mod tests` block, over a third of the file, to a sibling `db_ack_tests.rs`, matching the crate's own `merge/landing.rs`-style split elsewhere)
  - `crates/rune-tui/src/guard.rs` — 776 (grew with the disk-conflict Guard's [D]iscard/[M]erge ordering fix — `clear_guard` now runs after `merge::begin`'s refusal arms; split candidate unchanged: its `#[cfg(test)] mod tests` block is well over a third of the file — move it to a `#[path]`-included sibling `guard_tests.rs` so it keeps access to the private `set_guard`/`handle_disk_conflict_key` it exercises)
  - `crates/rune-tui/src/messages/mod.rs` — 561
  - `crates/rune-tui/src/render/filesearch.rs` — 546
  - `crates/rune-tui/src/rename.rs` — 559
  - `crates/rune-vfs/src/mem.rs` — 705 (`fail_resolve` and its tests pushed this further over)
  - `crates/rune-vfs/src/publish.rs` — 552 (already over before the narrow `put_force`/`put_if_absent` outcome types, which moved to a sibling `put_result.rs` rather than growing this further; split candidate: move the `#[cfg(test)] mod tests` block, well over half the file, to a `#[path]`-included sibling `publish_tests.rs` so it keeps access to the private `put_if_match`/`put_if_absent`/`finish_over_existing` it exercises)
  - `crates/rune-tui/src/dispatch.rs` — 540 (grew from 536)
  - `crates/rune-tui/src/document/mod.rs` — 683 (split candidate unchanged: move the `ReadOnly` enum plus its `impl` block, which don't depend on `Document`'s own fields, to a sibling `read_only.rs`)
  - `crates/rune-db/src/observation.rs` — 537 (split candidate: separate the observation row I/O — `scan_observation`, `insert_observation_row`, the query functions — from the stat-facts side — `StatFacts`, `ObservationMeta`, `stat_identity` — into a sibling `stat_facts.rs`)
  - `crates/rune-db/src/probe.rs` — 531 (the stat short-circuit and its confirmed/unconfirmed-history tests carry the file over; split candidate: move its own `#[cfg(test)]` module to a sibling `probe_tests.rs`, matching the crate's existing `materialize.rs`/`materialize_tests.rs` split)
  - `crates/rune-db/src/writer.rs` — 560 (split candidate: move the `execute_op` match into a sibling `writer_exec.rs`)
  - `crates/rune-cli/src/db_bootstrap.rs` — 507 (split candidate unchanged: move `bootstrap_untitled_db`/`ScratchDoc`/`DbBootstrapUntitled`/`degrade_untitled` to a sibling `db_bootstrap_untitled.rs`, matching the crate's own `bootstrap_tests.rs` split-out-of-`main.rs` pattern)
  - `crates/rune-cli/src/bootstrap_tests.rs` — 688 (test file; split candidate unchanged: move the launch-image-first tests (`launch_image_first_*`) plus `CountingReadVfs` to a sibling `bootstrap_tests_image.rs`, `#[path]`-included from `main.rs` the way `rune-db`'s `load_tests.rs` is from `load.rs`)
  - `crates/rune-tui/src/footer.rs` — 511
  - `crates/rune-tui/tests/opentabs.rs` — 550 (test file; grew from 479 in the Session-driver migration — `session.app_mut()` call verbosity plus rustfmt re-wrapping; split candidate: move the tab-limit/eviction tests to a sibling `opentabs_limit.rs` sharing `opentabs_common`)
  - `crates/rune-md/src/catalogue.rs` — 512
  - `crates/rune-fuzz/src/driver/mod.rs` — 562 (grew from 549: the `manual_clock` field and `Action::AdvanceClock` arm, plan WP8; split candidate: move the `'session` per-`Action` dispatch loop out of `run` into a sibling `action_loop.rs`, leaving `run` with setup, the end-of-session rules, and the `RunResult` assembly)
  - `crates/rune-tui/src/focus.rs` — 503
  - `crates/rune-tui/src/workspace/mod.rs` — 561 (pushed over by the `resolve_or_report` chokepoint added alongside the `resolve` signature change; split candidate: move the `#[cfg(test)] mod tests` block to a sibling test module)
  - `crates/rune-db/tests/multiprocess/scenarios.rs` — 509 (test file)
  - `crates/rune-tui/src/save/materialize_tests.rs` — 531 (test file; newly over — split candidate: move `snapshot_due_with_the_current_generation_enqueues_a_snapshot`/`snapshot_due_with_a_stale_generation_is_ignored`, which exercise `handle_snapshot_due` from `materialize_ack.rs` rather than this file's own CAS/publish path, to a sibling test module)
  - `crates/rune-tui/src/footer_hints.rs` — 546 (grew from 517: WP6 diff-view footer hints plus their test — split candidate: move its `#[cfg(test)] mod tests` block, over half the file, to a sibling `footer_hints_tests.rs`)
  - `crates/rune-tui/src/commands/mouse.rs` — 543 (was already 507, unrecorded; grew further with WP6's diff-left click handler — split candidate: move the `#[cfg(test)] mod tests` block, over a third of the file, to a sibling `mouse_tests.rs`)
  - `crates/rune-fuzz/src/driver/session.rs` — 592 (already over at 533 before a boot()-splitting cleanup added the named `new_app`/`open_seed_document`/`live_session`/`panicked_session` substeps; split candidate: move `Session::boot` and its four helpers, plus the `Seed`/`new_state` plumbing they share, to a sibling `boot.rs`, leaving `Session`'s post-boot methods here)
  - `crates/rune-tui/tests/diff_view.rs` — 602 (test file; newly over — the diff-view plan's own test suite grew through WP3-WP8: layout/alignment/intraline tests plus WP6's verb/chord/click tests all landed here; split candidate: move the verb and chord tests, `take_theirs_makes_the_region_same_and_undoes_in_one_step` through `click_in_the_left_pane_moves_the_right_pane_caret_to_the_aligned_line`, to a sibling `diff_view_verbs.rs`, leaving the layout/alignment/intraline tests here)
  - `crates/rune-fuzz/src/generate/cluster.rs` — 585 (grew from 505: plan WP8's `cluster_caret_history`/`cluster_advance_clock` plus WP9's merge-chord rework of `cluster_merge`'s own doc comment; split candidate: move the merge/highlight/multicursor cluster functions, `cluster_merge` through `cluster_multicursor`, to a sibling `cluster_scenarios.rs`, leaving the simpler single-shape clusters and `arb_cluster` itself here)
- **Wrong**: source files exceed the 500-line house rule, none ledgered. Five files dropped below 500 and are removed from this list: `crates/rune-tui/src/materialize_ack.rs` (305), `crates/rune-tui/src/materialize_ack/reactions.rs` (378), `crates/rune-fuzz/src/script/decode.rs` (413), `crates/rune-md/src/emit/mod.rs` (343), `crates/rune-syntax/src/wrap/mod.rs` (494). `crates/rune-syntax/src/syntax.rs` (466) and `crates/rune-tui/src/save/materialize.rs` (329) remain under the threshold from an earlier drop.
- **Instead**: split each per its own named candidate, once identified; comment purge (next entry) likely shrinks several below the threshold on its own.
- **Done when**: this list is empty (files legitimately re-measured after the comment purge, then split as needed).

### Comment purge (the refactor itself)
- **Where**: `crates/rune-tui` broadly — comments are roughly a third of the crate, rustdoc included
- **Wrong**: a paragraph-long justification comment indicts the code it justifies — the code is the refactor candidate, not the comment.
- **Instead**: apply the heuristic crate-wide: keep only complex-algorithm explanations (inside the function), third-party quirks that save real debugging time, and constraints no type/name/test can carry; delete the rest by cleaning the code they were defending.
- **Done when**: the purge has run and each surviving comment matches one of the three legitimate categories above.

### Comment citations of issue/plan numbers survive across most of rune-tui/rune-cli
- **Where**: a full sweep (`grep -rE 'issue #|Issue #|plan WP|plan decision|WP[0-9]'` over every file the Rust rewrite has ever touched in `crates/rune-tui`/`crates/rune-cli`) still turns up roughly 180 comments across ~30 files, concentrated in `db.rs`, `save.rs`, `save/materialize.rs`, `workspace/mod.rs`, `workspace/close.rs`, `materialize_ack/reactions.rs`, `merge/mod.rs`, `merge/landing.rs`, `explorer.rs`, and most of the `tests/db_wiring_*.rs`/`tests/image_document.rs`/`tests/rename_focus.rs` integration suites. This round swept only the review's own named locations plus every file this round's functional fixes touched.
- **Wrong**: these comments cite planning-doc identifiers (`issue #N`, `plan WPx.Sy`, `plan decision N`) that rot the moment the plan doc is gone — a bare name is search fodder, not a substitute for stating the invariant, per the constitution's comment article.
- **Instead**: strip the citation, keep the sentence self-contained, the same mechanical edit this round applied to its own set.
- **Done when**: the same grep across `crates/rune-tui`/`crates/rune-cli` returns nothing.
