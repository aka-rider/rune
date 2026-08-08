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
- **Where**: (a) `Document.save_in_flight` at `crates/rune-tui/src/document/mod.rs:130,341,371,383,403`, written directly by a test at `crates/rune-tui/src/merge/landing.rs:472`; (b) `is_dirty_cached` (`document/mod.rs:142`) vs `is_dirty` (`document/mod.rs:360-361`) vs `is_dirty_now` (`crates/rune-tui/src/materialize_ack.rs:408`)
- **Wrong**: (a) `save_in_flight` duplicates `save_pending.is_some()`; being `pub` let a test manufacture an "impossible" state by writing it directly. (b) two accessors exist where picking the wrong one is a per-call-site correctness decision, for a compare the code's own comment calls "length check + memcmp, microsecond-scale" — the cache buys only a staleness hazard.
- **Instead**: delete `save_in_flight` and derive it; delete the dirty cache or store a content hash instead.
- **Done when**: both fields are gone (or one is provably necessary and the other deleted).

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

## Mechanical

### Typed errors flattened to String
- **Where**: ~9 `map_err(|e| e.to_string())` at Cmd boundaries across `runtime/mod.rs`, `save.rs`, `trash.rs`, `rename_create.rs`, `graphics/*`; inside `rune-db::Error` (`crates/rune-db/src/error.rs:17,37,49,60`): `ReplayFailed(String)`, `CorruptPayload(String)`, `SessionEstablish(String)` stringify their sources while `Sqlite(rusqlite::Error)` proves the crate can hold typed sources
- **Wrong**: stringifying erases the `ErrorKind`/error type that `rune-vfs::WrappedIo` and `rusqlite::Error` deliberately preserve.
- **Instead**: typed variants; a small `Cause` enum in `Msg::Error`.
- **Done when**: no Cmd-boundary error is stringified before it reaches its handler, and `rune-db::Error`'s String variants hold typed sources.

### Stale/false comments (provable lies)
- **Where**: "nightly-only" claims (see the `char_boundary` entry above); `crates/rune-cli/src/open.rs:150`, `crates/rune-tui/tests/db_wiring_hydrate.rs:4` (cite deleted per-crate `TODO.md`s); `crates/rune-syntax/src/wrap/width.rs:93`, `crates/rune-tui/tests/tui_render_text.rs:387` (cite a nonexistent `TODO/TODO.md`)
- **Wrong**: comments cite functions and files that no longer exist.
- **Instead**: fix or delete each citation when touched (per house rule, no `path:line` in comments either).
- **Done when**: no comment in the tree cites a nonexistent symbol or deleted file.

### O(file) per keystroke is the deliberate design ceiling
- **Where**: `crates/rune-core/src/buffer/mod.rs`, `crates/rune-core/src/buffer/lineindex.rs`, `crates/rune-tui/src/commands/edit_core.rs`, `crates/rune-tui/src/materialize_ack.rs`; perf-guarded by `crates/rune-tui/tests/perf_guard.rs:92` (`keystroke_view_cost_under_budget_on_a_5k_line_code_document`)
- **Wrong**: full content copy + full line-index clone + full memcmp + journal clones per edit batch; does not scale past the guard fixture's size.
- **Instead**: a rope with the same value-semantics facade, if the ceiling is ever hit in practice.
- **Done when**: not currently actionable — record only; revisit if the perf guard's fixture size stops matching real documents.

### Files over 500 lines
- **Where** (recomputed from the live tree; comment purge below will change these numbers):
  - `crates/rune-db/src/sync.rs` — 873 (new WP-C: own-history echo + its tests pushed this over; split candidate: move the `#[cfg(test)]` module to a sibling `sync_tests.rs`, the `materialize.rs`/`materialize_tests.rs` pattern this crate already uses)
  - `crates/rune-tui/src/explorer_preview/tests.rs` — 868 (test file)
  - `crates/rune-tui/src/global.rs` — 809
  - `crates/rune-tui/src/pane.rs` — 800
  - `crates/rune-tui/src/layout.rs` — 754
  - `crates/rune-merge/src/hunks.rs` — 684 (grew past the threshold in WP-D fixing the anchoring bug; the `#[cfg(test)] mod tests` block is over half the file — split candidate: move it to a `#[path]`-included sibling test module so it keeps access to the private `parse_hunks`/`anchor_section` it exercises)
  - `crates/rune-tui/src/runtime/mod.rs` — 672
  - `crates/rune-fuzz/src/generate/palette.rs` — 660
  - `crates/rune-tui/src/app.rs` — 625 (grew further in the G7 fix adding the `file_bindings` shared-baseline map's own doc comment)
  - `crates/rune-tui/tests/rename_focus.rs` — 606 (test file)
  - `crates/rune-tui/src/filesearch/tests.rs` — 599 (test file)
  - `crates/rune-tui/src/merge/landing.rs` — 598 (grew further in the G7 fix rewiring the absent-ancestor dispatch onto `ancestor_rung` and moving `advance_expect_obs` onto the shared `FileBinding`; split candidate unchanged: move the `#[cfg(test)] mod tests` block, over a third of the file, to `crates/rune-tui/tests/merge_landing_unit.rs` or keep it `#[path]`-included from `landing.rs` if it needs the private fns it exercises)
  - `crates/rune-tui/src/db.rs` — 512 (the review-fixes chokepoint pair `App::doc_file_binding`/`doc_file_binding_mut`/`doc_db_id` pushed this over; split candidate: move the `FileBinding`/`DocDb` type definitions to a sibling `db_types.rs`, keeping the `Db`/writer-bridge wiring here)
  - `crates/rune-tui/src/guard.rs` — 558 (crossed the threshold in WP-D adding the `DiskConflict` convergence self-retraction and its tests; split candidate: same as above, its `#[cfg(test)] mod tests` block is roughly a fifth of the file)
  - `crates/rune-tui/src/messages/mod.rs` — 557
  - `crates/rune-md/src/emit/mod.rs` — 556
  - `crates/rune-tui/src/render/filesearch.rs` — 546
  - `crates/rune-tui/src/rename.rs` — 544
  - `crates/rune-vfs/src/mem.rs` — 536
  - `crates/rune-tui/src/dispatch.rs` — 526
  - `crates/rune-db/src/observation.rs` — 528 (new WP-C: the `supersedes` lineage-edge computation pushed this over; split candidate: move its own `#[cfg(test)]` module to `observation_tests.rs`)
  - `crates/rune-db/src/probe.rs` — 528 (the stat short-circuit and its confirmed/unconfirmed-history tests carry the file over; split candidate: move its own `#[cfg(test)]` module to a sibling `probe_tests.rs`, matching the crate's existing `materialize.rs`/`materialize_tests.rs` split)
  - `crates/rune-db/src/bracket.rs` — 569 (the shrink-hypothesis-then-validate tests for the review-fixes shrink-confirmation gate, plus a persistent-stat-failure `Vfs` test fixture, pushed this over; split candidate: move its own `#[cfg(test)]` module to a sibling `bracket_tests.rs`, same pattern)
  - `crates/rune-syntax/src/wrap/mod.rs` — 513
  - `crates/rune-tui/src/footer.rs` — 512
  - `crates/rune-md/src/catalogue.rs` — 512
  - `crates/rune-fuzz/src/driver/mod.rs` — 508
  - `crates/rune-tui/src/focus.rs` — 506
  - `crates/rune-syntax/src/syntax.rs` — 505
  - `crates/rune-fuzz/src/script/decode.rs` — 503
  - `crates/rune-tui/src/save/materialize.rs` — 523 (crossed the threshold in the G7 fix: `materialize_now` now reads `expect_obs` off `App::file_bindings` instead of `DocDb` directly, then grew further in the review-fixes pass refusing a missing file binding explicitly instead of a `0` sentinel; split candidate: move `run_materialize_vfs`'s `force_publish`/`capture_and_swap_publish` helpers to a sibling `force_publish.rs`)
- **Wrong**: 31 source files exceed the 500-line house rule, none ledgered.
- **Instead**: split each per its own named candidate, once identified; comment purge (next entry) likely shrinks several below the threshold on its own.
- **Done when**: this list is empty (files legitimately re-measured after the comment purge, then split as needed).

### Comment purge (the refactor itself)
- **Where**: `crates/rune-tui` broadly — comments are roughly a third of the crate, rustdoc included
- **Wrong**: a paragraph-long justification comment indicts the code it justifies — the code is the refactor candidate, not the comment.
- **Instead**: apply the heuristic crate-wide: keep only complex-algorithm explanations (inside the function), third-party quirks that save real debugging time, and constraints no type/name/test can carry; delete the rest by cleaning the code they were defending.
- **Done when**: the purge has run and each surviving comment matches one of the three legitimate categories above.
