# TODO — Refactor Ledger

Reinstated 2026-08-07 from a three-audit sweep of the codebase. This is an
inventory of known violations of `CONSTITUTION.md`'s governing rules and of
non-idiomatic implementations with better alternatives available; it seeds a
future refactor plan. New code must not repeat these patterns even where
legacy code still does. Fixes land with tests per the constitution, and an
entry is deleted in the same commit that fixes it.

## Data-safety

### rune-merge scrapes diffy's rendered conflict markers
- **Where**: `crates/rune-merge/src/hunks.rs:41` (`merge_bytes` call), `crates/rune-merge/src/hunks.rs:111-192` (`parse_diff3`, `find_subslice`, `anchor_section`)
- **Wrong**: `parse_diff3` re-parses diffy's rendered `<<<<<<<`/`=======`/`>>>>>>>` output by line-prefix matching, and `anchor_section` re-anchors each section by first-occurrence substring search. A document already containing marker-shaped lines can mis-segment (bytes reassigned across Clean/Conflict hunks) or mis-anchor on repeated lines — both silent, on a data-destructive path.
- **Instead**: get structured hunks instead of parsing display form; at minimum scan inputs for the longest marker run and call `set_conflict_marker_length` so collision is unrepresentable, and anchor by consumed-byte accounting, not content search.
- **Done when**: conflict segmentation no longer depends on parsing diffy's rendered text, or marker-length collision is provably unrepresentable and anchoring is position-accounted.
- **Update 2026-08-07 (WP-D)**: a real anchor failure (diffy's diff3 marker text newline-terminates a section's final line even when the source input has no trailing newline there) was root-caused and fixed with a bounded trailing-newline retry, and a failed anchor now widens only its own run of conflicts instead of collapsing the whole file (see `parse_hunks`). The repeated-marker-line collision risk this entry originally raised is untouched by that fix and remains open.

### Same-session reopen after an external rewrite still adopts stale content
- **Where**: `crates/rune-db/src/load.rs`, `load`'s `has_hist` branch.
- **Wrong**: when a tab that already has journal history in this session is reopened after some OTHER tool rewrote the file on disk, `load` returns this session's own journal reconstruction — the pre-rewrite content — never the new disk content, even though the session made no edits of its own. A fix was tried that swapped in the reconstructed disk content directly when it disagreed with this session's `saved_obs` baseline; it was withdrawn because returning that string without also journaling a matching bridge edit leaves the buffer and this session's journal disagreeing — the next edit journals at offsets valid only for the buffer, so `recover_document` either dies with "edit out of bounds" or the durable record silently corrupts.
- **Instead**: re-anchor this session's own journal to the new disk content before returning it — a bridge edit (or a fresh anchor snapshot) the way `load_anchor::anchor_first_load` does for the cross-session case — never swap the returned string alone.
- **Done when**: a same-session reopen after an external rewrite reflects the new disk content AND this session's own journal reconstruction agrees with it (the assertion `dead_session_with_no_edit_yields_disk_and_the_new_sessions_journal_agrees` pins for the cross-session case has a same-session equivalent that passes).

### `published_not_durable` honored at only 1 of 4 publish sites
- **Where**: contract at `crates/rune-vfs/src/lib.rs:88`; honored at `crates/rune-tui/src/save/materialize.rs:294`; ignored at `crates/rune-db/src/rename_replace.rs:72`, `crates/rune-db/src/rename_bind.rs:35-40`, `crates/rune-tui/src/rename_create.rs:158`
- **Wrong**: the predicate means the swap/rename already took effect and callers must never remove the temp (sole surviving copy of displaced bytes). Three call sites propagate the raw `io::Error` (or a stringified generic arm) without checking it, so a physically-successful rename can be reported as failed while UI/DB state disagrees with disk.
- **Instead**: every publish site branches on `published_not_durable` before deciding what to do with the temp, same as `materialize.rs`.
- **Done when**: all four sites branch on the predicate identically.

## Architecture

### Shadow state
- **Where**: `is_dirty_cached` (`document/mod.rs:142`) vs `is_dirty` (`document/mod.rs:360-361`) vs `is_dirty_now` (`crates/rune-tui/src/materialize_ack.rs:408`)
- **Wrong**: two accessors exist where picking the wrong one is a per-call-site correctness decision, for a compare the code's own comment calls "length check + memcmp, microsecond-scale" — the cache buys only a staleness hazard.
- **Instead**: delete the dirty cache or store a content hash instead.
- **Done when**: one of the two is deleted.
- **Update**: the `save_in_flight` half of this entry is resolved — `Document.save_in_flight` is now a derived accessor over the `SaveState` machine (`document/save_state.rs`), and no test can manufacture an impossible in-flight state by writing a field directly.

### Sentinel-value class
- **Where**: `crates/rune-syntax/src/syntax.rs:56` (`CellMap = Vec<i64>`, -1 = decorative); `crates/rune-tui/src/render/cell.rs:38,132` (`buf_offset: i64`, -1, guarded by `< 0` at ~6 render sites then `as usize`); `crates/rune-md/src/table/mod.rs:36` (`buf: i64`); `crates/rune-tui/src/app.rs:337,409` (`root: PathBuf`, empty = unresolved); `crates/rune-tui/src/app.rs:82-92` (`frame_height/width: u16`, 0 = no resize yet); `crates/rune-tui/src/filesearch/rank.rs:112` and `crates/rune-tui/src/messages/mod.rs:371` (`unwrap_or(usize::MAX)`)
- **Wrong**: each type can represent an invalid state (negative offset, unresolved root, unsized frame) as a valid-looking value; a forgotten guard is a silent logic bug, not a compile error.
- **Instead**: `Option<usize>` or an enum at each site.
- **Done when**: no sentinel value stands in for "absent"/"unresolved" in these types.

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

### Multi-meaning `None`s in the highlight reply protocol
- **Where**: `crates/rune-tui/src/runtime/mod.rs:187-190` (`Msg::Highlighted { result: Option<HighlightReply> }`), `crates/rune-tui/src/highlight/mod.rs:122` (`payload: Option<RegionPayload>`)
- **Wrong**: `result: None` means "carry forward"; within a reply, per-region `payload: None` means "keep channels" while `Some(empty)` means "clear" — decoded by convention and comments, not the type.
- **Instead**: an explicit `CarryForward`/`Replace` enum.
- **Done when**: the reply protocol has no dual-meaning `None`.

### A tab eaten by container indentation shifts every column on its line
- **Where**: `sourcepos_to_range`, `offset_of_column` and `indent_bears_tab` in `crates/rune-md/src/parse/mod.rs`; the shift is pinned by `partially_consumed_tab_on_a_lazy_line_shifts_columns` and `partially_consumed_tab_shift_survives_as_a_shorter_range` in `crates/rune-md/tests/spike_sourcepos.rs`
- **Wrong**: comrak gives each node a line and a column, and those columns count bytes — except on one kind of line. A container such as a list item eats part of the line's leading tab. The line then continues an already-open block without repeating that block's own prefix. comrak fills the tab's uneaten remainder with spaces inside the block's content, while the byte offset has already stepped over the whole tab. Every column reported on that line therefore comes back shifted right. The shift equals the container's indentation, which the reported position does not carry, so a column on its own cannot be corrected — only bounded by the tab's width. `sourcepos_to_range` resolves each column against its own line's bytes and clamps inside that line, which keeps every offset on the right line and on a character boundary. Within the line an offset can still be a few columns off, so a style boundary can land in the wrong place. Byte accounting is intact: ten shifted shapes pass the per-line coverage and duplicate-content checks, so no byte is dropped or drawn twice. Surfaced by a fuzz catch where the shift pushed an offset into the middle of a multi-byte character (issue #94).
- **Instead**: rebuild the indentation the shift is made of by walking the enclosing block's own prefix on that line, instead of trusting the column. Or take an upstream comrak change that reports byte columns on these lines.
- **Done when**: a node on a tab-indented lazy continuation line converts to its exact byte range, and the two tests named above assert exactness instead of recording the shift as bounded.

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
  - `crates/rune-db/src/sync.rs` — 778 (split candidate: move the `#[cfg(test)]` module to a sibling `sync_tests.rs`, the `materialize.rs`/`materialize_tests.rs` pattern this crate already uses)
  - `crates/rune-tui/src/explorer_preview/tests.rs` — 1060 (test file)
  - `crates/rune-tui/src/global.rs` — 767
  - `crates/rune-tui/src/pane.rs` — 858
  - `crates/rune-tui/src/layout.rs` — 736
  - `crates/rune-merge/src/hunks.rs` — 684 (the `#[cfg(test)] mod tests` block is over half the file — split candidate: move it to a `#[path]`-included sibling test module so it keeps access to the private `parse_hunks`/`anchor_section` it exercises)
  - `crates/rune-tui/src/runtime/mod.rs` — 691
  - `crates/rune-fuzz/src/generate/palette.rs` — 659
  - `crates/rune-tui/src/app.rs` — 609
  - `crates/rune-tui/tests/rename_focus.rs` — 606 (test file)
  - `crates/rune-tui/src/filesearch/tests.rs` — 599 (test file)
  - `crates/rune-tui/src/merge/landing.rs` — 600 (split candidate unchanged: move the `#[cfg(test)] mod tests` block, over a third of the file, to `crates/rune-tui/tests/merge_landing_unit.rs` or keep it `#[path]`-included from `landing.rs` if it needs the private fns it exercises)
  - `crates/rune-tui/src/db.rs` — 579 (split candidate unchanged: move the `FileBinding`/`DocDb` type definitions to a sibling `db_types.rs`, keeping the `Db`/writer-bridge wiring here)
  - `crates/rune-tui/src/db_ack.rs` — 689 (the binding/replica-seam work — `Replica::take_pending`'s call sites, the hardlink-fork load warning and its tests — has pushed this further over collectively, no single change owning it; split candidate unchanged: move the `#[cfg(test)] mod tests` block, over a third of the file, to a sibling `db_ack_tests.rs`, matching the crate's own `merge/landing.rs`-style split elsewhere)
  - `crates/rune-tui/src/materialize_ack/reactions.rs` — 517 (split candidate: the module already has a sibling `reactions_tests.rs` — move `close_if_pending`/`quit_if_pending`/`retire_quit_wait`, which don't depend on `handle_materialize_ack`'s own locals, to a sibling `quit.rs`)
  - `crates/rune-tui/src/guard.rs` — 754 (split candidate unchanged: its `#[cfg(test)] mod tests` block is well over a third of the file — move it to a `#[path]`-included sibling `guard_tests.rs` so it keeps access to the private `set_guard`/`handle_disk_conflict_key` it exercises)
  - `crates/rune-tui/src/messages/mod.rs` — 557
  - `crates/rune-tui/src/render/filesearch.rs` — 546
  - `crates/rune-tui/src/rename.rs` — 555
  - `crates/rune-vfs/src/mem.rs` — 706 (`fail_resolve` and its tests pushed this further over)
  - `crates/rune-tui/src/dispatch.rs` — 539
  - `crates/rune-tui/src/document/mod.rs` — 676 (split candidate unchanged: move the `ReadOnly` enum plus its `impl` block, which don't depend on `Document`'s own fields, to a sibling `read_only.rs`)
  - `crates/rune-db/src/observation.rs` — 545 (split candidate: separate the observation row I/O — `scan_observation`, `insert_observation_row`, the query functions — from the stat-facts side — `StatFacts`, `ObservationMeta`, `stat_identity` — into a sibling `stat_facts.rs`)
  - `crates/rune-db/src/probe.rs` — 528 (the stat short-circuit and its confirmed/unconfirmed-history tests carry the file over; split candidate: move its own `#[cfg(test)]` module to a sibling `probe_tests.rs`, matching the crate's existing `materialize.rs`/`materialize_tests.rs` split)
  - `crates/rune-db/src/writer.rs` — 552 (split candidate: move the `execute_op` match into a sibling `writer_exec.rs`)
  - `crates/rune-cli/src/db_bootstrap.rs` — 509 (split candidate unchanged: move `bootstrap_untitled_db`/`ScratchDoc`/`DbBootstrapUntitled`/`degrade_untitled` to a sibling `db_bootstrap_untitled.rs`, matching the crate's own `bootstrap_tests.rs` split-out-of-`main.rs` pattern)
  - `crates/rune-cli/src/bootstrap_tests.rs` — 688 (test file; split candidate unchanged: move the launch-image-first tests (`launch_image_first_*`) plus `CountingReadVfs` to a sibling `bootstrap_tests_image.rs`, `#[path]`-included from `main.rs` the way `rune-db`'s `load_tests.rs` is from `load.rs`)
  - `crates/rune-syntax/src/wrap/mod.rs` — 509
  - `crates/rune-tui/src/footer.rs` — 512
  - `crates/rune-md/src/catalogue.rs` — 512
  - `crates/rune-fuzz/src/driver/mod.rs` — 548 (pushed further over by the session-setup panic guard; split candidate: move the `'session` per-`Action` dispatch loop out of `run` into a sibling `action_loop.rs`, leaving `run` with setup, the end-of-session rules, and the `RunResult` assembly)
  - `crates/rune-tui/src/focus.rs` — 506
  - `crates/rune-fuzz/src/script/decode.rs` — 503
  - `crates/rune-tui/src/materialize_ack.rs` — 577 (split candidate unchanged: move `record_outcome`/`record_orphan_outcome`/`RecordTarget` to a sibling `record.rs`)
  - `crates/rune-tui/src/workspace/mod.rs` — 563 (pushed over by the `resolve_or_report` chokepoint added alongside the `resolve` signature change; split candidate: move the `#[cfg(test)] mod tests` block to a sibling test module)
  - `crates/rune-db/tests/multiprocess/scenarios.rs` — 507 (test file)
- **Wrong**: source files exceed the 500-line house rule, none ledgered. (`crates/rune-syntax/src/syntax.rs` dropped to 467 lines and is no longer over — removed from this list. `crates/rune-tui/src/save/materialize.rs` at 320 lines is also not over.)
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
