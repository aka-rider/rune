# TODO — Rust workspace

Scope: the Rust implementation at the repo root (`crates/…`) and the cross-language parity harness (`scripts/parity/…`).
The Go reference implementation keeps its own list in `golang/TODO.md`.

## Comment citations of Go source locations (recorded 2026-07-28, at the rust/golang swap)

`CLAUDE.md`'s new house rule — never cite a file path, line number, or symbol location in a code comment — was applied to the ~117 full-path Go citations (`pkg/…`, `cmd/rune`, `internal/…`) under `crates/`, which the move to `golang/` would otherwise have made wrong. **Still open:** ~242 bare Go-filename citations with line numbers (`edit_primitives.go:51,86`, `workspace_view.go:327-330`, `breadcrumb.go:56-119`, …) across `crates/`. The move didn't invalidate them any further than they already were, and rewriting each one needs a judgement call about what the sentence is actually claiming, so they were left in place rather than mangled in bulk. Erase them opportunistically when touching the surrounding code.

## House Rule clarification: bare-filename citations are tolerated-but-discouraged, not banned (recorded 2026-07-28, `fix-fuzz` branch code review)

`CLAUDE.md`'s House Rules bullet on comment citations was ambiguous about whether a bare filename (no `:line`) counts as a banned "file path" — read literally, it did, yet the codebase carries roughly 238 bare-filename references in comments (`palette.rs`, `cluster.rs`, …), 218 of which predate `fix-fuzz`. The rule's own stated rationale is ROT: a `path:line` citation rots the moment either file moves (verified repeatedly on this branch), while a bare filename or module path is far less rot-prone — it survives line churn, only breaking on a rename. The rule text is now amended to say this explicitly: `path:line` citations are banned outright, a bare filename/module path is tolerated but discouraged and should be replaced with a description of the invariant when touched. The existing ~238 references were deliberately **not** mass-edited to close this finding — the same "needs a judgement call per comment" reasoning as the Go-citations entry above applies. Replace opportunistically when touching the surrounding code, per the amended rule.

## rust port — vfs primitive shape vs §1.4.10 (recorded 2026-07-26, from Opus review of rust/wp2-vfs)

- ~~`crates/rune-core/src/vfs` collapses `Exchange` + temp-unlink into a single `save_atomic`, destroying the displaced bytes *inside* the primitive. No caller can ever re-read and hash what the swap displaced — this forecloses `Materialize` step 4 (§1.4.10 "capture before discard, guaranteed by mechanism"). §1.4.1 names `Exchange` and `RenameExcl` as two separate primitives with the caller choosing; the internal `path.exists()` choice also cannot express §1.4.4's "never silently create on an overwrite-intent save".~~
- ~~**Resolve before the Phase-2 docstate port**: split the Rust trait into `exchange`/`rename_excl` primitives (mirroring Go `pkg/vfs`) with the displaced-content temp surviving long enough for the caller to hash/blob it, or add an explicit capture callback to `save_atomic`.~~
- **RESOLVED:** the trait (now `crates/rune-vfs`, not `rune-core/src/vfs` — that path no longer exists) split into `fn exchange(&self, a: &Path, b: &Path)` (`rune-vfs/src/lib.rs:83`) and `fn rename_excl(&self, old: &Path, new: &Path)` (`:89`), backed by `libc::RENAME_SWAP`/`libc::RENAME_EXCL` in `rune-vfs/src/disk.rs:114-120`; `save_atomic` survives only as a demoted default-body convenience (`lib.rs:127`) that says plainly it cannot satisfy §1.4.10 alone. The capture-before-discard this entry demanded is realised at `crates/rune-db/src/materialize.rs:199-223`: it reads and hashes the displaced bytes (`vfs.read(&temp)`, `:201`) and only removes the temp (`vfs.remove(&temp)`, `:223`) after the DB transaction has committed.
- Related deferred item: `EOPNOTSUPP → ErrUnsupported` mapping for SMB/NFS mounts (plan Spike 4).

## rust port — deferred hygiene items (recorded 2026-07-26, stage-2b repair rounds)

- `crates/rune-md/tests/conceal_roundtrip.rs` is 1407 lines (§1.6 limit 500). Split into focused test modules with a shared helper crate/module when next touched.
- Upstream comrak bug (worked around, not reported): comrak 0.54's internal line counter desyncs after a wikilink containing an embedded newline, corrupting sourcepos for later inline siblings in the paragraph. `build_inlines` detects and defends (per-line rebuild). Consider minimizing + reporting upstream; remove the workaround when fixed.
- rune-md residual fuzz-line-noise double-claims (4 docs per 214k shipped-build, 182 strict): fence/backtick + blockquote compositions like ">c\n`\n>`" and "t\n  -```\n*```\n>" — focused-only 1-byte caret skews, absorbed by the single-claim chokepoint in shipped builds. Repro/shrinker: scratchpad probe-strict harnesses (stage-2b review). Fix the producer when next in rune-md/parse.
- Upstream comrak sourcepos self-inconsistencies (independently confirmed by AST probe): sibling Text nodes sharing one byte range, ranges not containing their own literal, children escaping parents — triggered by lone-CR / raw-tab × lazy continuation. 18 strict panics + 2 shipped violations per 214k fuzz docs, all upstream, gracefully absorbed. Three repros in crates/rune-md/TODO.md. Action: minimize + file against comrak; keep any strict-invariants CI job NON-BLOCKING (or exclude the three repros) until fixed upstream.
- Inherited Go parity papercut (both implementations): cut with nothing to cut (empty last line / empty buffer) journals a zero-width step, bumps version, and marks the doc dirty — a clean file then warns "unsaved changes" until undo. Go's ApplyEdits (buffer.go:115-145) likewise doesn't skip zero-width batches. Fix in both: skip empty edit batches at the commit chokepoint.
- rune-md perf harness: `build_5k_doc` is duplicated verbatim between benches/parse_bench.rs and tests/perf_guard.rs — the guard defends the number the bench measures, so silent divergence would decouple them. Extract a shared generator (include! of a common file) when next touched.

## rust port — §1.6 file-size overages (recorded 2026-07-27)

### recorded 2026-07-29 by the `ts-int` tree-sitter integration merge

- `crates/rune-tui/src/app.rs` is 501 lines (§1.6 limit 500) — one line over.
  `ts-int` had deliberately brought it to 483 by extracting `update_inner`/
  `handle_key`/`handle_editor_key`/`handle_db_event` into `dispatch.rs` and
  `schedule_highlight` into `highlight.rs` (that plan's own "app.rs < 500"
  done-when). The merge re-added `rr`'s `db_load_versions` and `root` fields,
  their doc comments and initializers, and `set_root` — pushing it back over by
  one. Deliberately NOT fixed by trimming the two breadcrumb comments that
  explain where the extracted functions went: §1.6 is satisfied by splitting a
  file, never by deleting the comments that make the split navigable. Move
  `set_root` and the workspace-root field onto a small `workspaceroot` state
  struct when next touched.

### recorded 2026-07-29 by the keystroke-latency plan (WP16.S5)

- `crates/rune-tui/src/app.rs` is 509 lines (§1.6 limit 500; was 501 immediately
  before this work — see the `ts-int` entry above) — grown 8 lines by the
  `snapshot_timer: Arc<runtime::SnapshotTimer>` field, its doc comment, and its
  `App::new` initializer (WP16.S5 replaced the per-message thread-per-keystroke
  snapshot-autosave debounce `Cmd` spawn with one rearmable timer thread per
  app, which `App` now has to hold a handle to). The new logic itself lives
  entirely in the new `runtime/snapshot_timer.rs` (236 lines, its own budget);
  `app.rs` gained only the one field a struct necessarily needs. Same
  deferred fix every `app.rs` entry above names: extract the four-stage
  `handle_key`/`handle_editor_key`/`handle_db_event` trio into their own
  module; out of scope for this plan (WP16 is the keystroke-latency package,
  not the `app.rs` split).
- `crates/rune-md/tests/table_render.rs` is 530 lines (§1.6 limit 500; was 470
  on `rr` and 425 on `ts-int`) — both branches independently found and pinned
  the SAME Wrapped-table border-width bug with complementary tests, and the
  merge keeps both (`rr`'s asserts exactly one top/bottom border row survives a
  multi-visual-row wrap; `ts-int`'s asserts the border width equals the
  constrained layout width). Neither is redundant. Split by table layout mode
  (Grid / Wrapped / Pivoted) when next touched.


CONSTITUTION.md §1.6: "One primary type per file; decompose any file past 500 LoC." Decomposing any of these is out of scope for the work that surfaced them; recording per the project convention that a pre-existing issue is never silently skipped.

**RESOLVED (2026-07-28), five pure mechanical splits, hoisted forward out of the tree-sitter plan as WP5.S5/WP5.S6/WP7.S1:** every still-open `app.rs`/`render.rs`/`script.rs`/`driver.rs`/`generate.rs` entry below is struck. `crates/rune-tui/src/app.rs` (779 lines) had `update_inner`/`handle_key`/`handle_editor_key`/`handle_db_event` extracted into a new `dispatch.rs` (app.rs now 463, dispatch.rs 338). `crates/rune-tui/src/render.rs` (503 lines) became `render/mod.rs` + a new `render/overlay.rs` carrying `apply_cursor_overlays`/`highlight_selection`/`place_caret` (438 + 84), with every external `render::` path unchanged. `crates/rune-fuzz/src/script.rs` (634 lines) became `script/mod.rs` + `script/encode.rs` + `script/decode.rs` (223 + 116 + 320). `crates/rune-fuzz/src/driver.rs` (550 lines) became `driver/mod.rs` + `driver/checks.rs` carrying the sampled checkers, `should_sample`, and the end-of-session undo/redo drive (400 + 181). `crates/rune-fuzz/src/generate.rs` (539 lines) became `generate/mod.rs` + `generate/palette.rs` + `generate/cluster.rs` (40 + 335 + 213). Every public path (`rune_fuzz::script::{encode,decode}`, `rune_fuzz::driver::run`, `rune_fuzz::generate::{arb_session,TYPE_PALETTE}`) is unchanged via re-export, so no test file needed editing. Zero behaviour change.

- **RESOLVED by the rust-port Phase 2 WP1 document-map refactor (2026-07-27):** `crates/rune-tui/src/app.rs` was 1165 lines (the two entries below, now removed). WP1 moved every per-document field onto the new `Document`/`DocumentId` (`document.rs`), extracted the save/ack/dirty flow into `save.rs`, and moved the pure-public-API unit tests that used to live in `app.rs`/`save.rs` out to `tests/app_quit_and_dispatch.rs`/`tests/save_flow.rs` — `app.rs` is now 494 lines, under the §1.6 limit.
- ~~`crates/rune-tui/src/app.rs` is 516 lines (§1.6 limit 500; was already exactly 500 — zero headroom — after WP2) — grown by plan WP3's designated insertion points: the `App.modal: Option<banner::Modal>` field, stage 1 of `handle_key` delegating to `banner::handle_key`, `sync_view` also re-syncing the modal document, and `Msg::Error` routing through `banner::report_error` instead of `set_status`. All new logic beyond these few call sites lives in the new `banner.rs` (WP3's own budget, 302 lines) — `app.rs` itself only gained the minimum wiring the plan's marked insertion point required, already trimmed to terse comments. Split (e.g. `handle_key`/`handle_editor_key`/`handle_db_event` into their own module, mirroring `pane.rs`'s WP2 extraction) when next touched.~~
  - ~~`crates/rune-tui/src/app.rs` is 848 lines (§1.6 limit 500) — grown by this work, from 813 to 848 (~35 lines) by the `CmdKind` refactor.~~
  - ~~`crates/rune-tui/src/app.rs` grew to 1165 lines after the rune-db wiring merge (§1.6 limit 500; was 848).~~
- `crates/rune-tui/src/commands/edit.rs` is 695 lines (§1.6 limit 500; was 601 before WP1) — **grown further to 695 by commit `779e5e5` ("WP1 — fallible doc accessors, NonZero id counter")**, a growth the TODO entry here previously stopped tracking at 660 (this entry corrects that drift; found stale during a later review). Still pre-existing-overage territory, not newly introduced by any one change; the unit tests (a large share of the 695 lines) only touch this module's own `pub` functions and could move to an integration test the same way `app.rs`'s did, but the split-by-command-family fix already on file below is the more useful eventual shape. Split by command family when next touched.
- `crates/rune-core/src/buffer.rs` is 635 lines (§1.6 limit 500; was 617) — grown ~18 lines by commit `a1fe09d`'s `line_col_to_offset` char-boundary snap (plus its `floor_char_boundary` helper): a multicursor add replaying a remembered byte column onto another line could land mid-UTF-8 when that line held wide characters, an invariant violation the session fuzzer only surfaced once table seeds pushed a document into this path. Pre-existing overage (recorded above at 568 lines by WP2), not newly created by the table work itself. Split by concern when next touched.
- `crates/rune-tui/src/commands/nav.rs` is 584 lines (§1.6 limit 500; was 532 immediately before this work) — grown ~52 lines by the editor-MVP plan's WP9.S1 Unicode word classifier (`char_class`'s rewrite, the new `is_word_forming` probe, and two new Cyrillic/mixed-script regression tests). Pre-existing overage (recorded above at 530 lines from an earlier package); this work only touched `char_class` and its own doc comment, per the WP9 brief's explicit instruction to keep this file's diff to the word-motion functions only (a different worker was adding scroll commands here concurrently). Split by command family when next touched.
- `crates/rune-tui/src/keymap.rs` is 528 lines (§1.6 limit 500; was 472 immediately before this work, already under budget) — grown ~56 lines by the editor-MVP plan's WP9.S2/S3 `Command` variants (`DeleteWordLeft`/`Right`, `DeleteLine`, `MoveLineUp`/`Down`, `CloneLineUp`/`Down`, `AddCursorAbove`/`Below`) and the new `resolve_vertical` helper + `Backspace`/`Delete`/`'k'` resolver arms binding them. This is the file's first crossing of the §1.6 ceiling. Split when next touched: `resolve_directional`/`resolve_plain_or_shift`/`resolve_vertical` (the three chord-shape helpers) could move to a sibling module alongside `Command` itself, mirroring the `binding.rs`/`global.rs` extraction this file already went through once.
- **RESOLVED (2026-07-28) by the space-leader plan's WP1 split into `binding.rs` + `global.rs`:** `crates/rune-tui/src/keymap.rs` is 595 lines (§1.6 limit 500; was 428) — grown by the WP2.S3 `KeyPattern`/`Binding<C>`/`resolve_in`/`KeyOutcome`/`GlobalCommand`/`GLOBAL_BINDINGS` additions (per the WP2 task brief: "In keymap.rs add: ..."). The new `resolve_in`/`GLOBAL_BINDINGS` test coverage was deliberately kept OUT of this file (`tests/keymap_global.rs`) to limit the growth; further reduction would mean moving `GlobalCommand`/`GLOBAL_BINDINGS` themselves out of `keymap.rs` against the brief's explicit instruction. Split (e.g. the generic `KeyPattern`/`Binding`/`resolve_in`/`KeyOutcome` trio into their own module, reusable by WP4/WP5's `EXPLORER_BINDINGS`/`TABS_BINDINGS`) when next touched.
- `crates/rune-db/tests/multiprocess.rs` showed one transient timeout under heavy sandbox load (marker-file poll deadline; rusqlite 5s busy_timeout × 5 backoff attempts can approach the 30s ceiling worst-case). Passed repeatedly otherwise. If it recurs in CI: raise the scenario deadline or serialize the four scenarios.
- ~~`crates/rune-tui/src/app.rs` is 511 lines (§1.6 limit 500; was 500) — grown ~11 lines by the WP4 Explorer wiring (the `explorer: Explorer` field, its `App::new` initializer, the `Msg::DirLoaded` dispatch arm, and the `Pane::Explorer` stage-3 arm). `app.rs` was already sitting exactly at the 500-line ceiling before this work (see the WP1 entry above); a further extraction (e.g. the four-stage `handle_key`/`handle_editor_key` pair into their own module) would be needed to bring it back under budget. Deferred — out of scope for WP4 itself.~~
- ~~`crates/rune-fuzz/src/script.rs` is 608 lines (§1.6 limit 500; was 501, already pre-existing-overage territory before this work) — grown ~107 lines by the WP4.S6 `dirloaded`/`dirloaded-entry` multi-line grammar (`encode_action`'s new arm, `parse_dir_loaded`/`parse_dir_entry`, the `Peekable`-based `decode` restructure, and the round-trip test coverage). A `DirEntry`'s `name` can contain a literal space, so this needed either a second escaping scheme or the multi-line continuation-record shape actually used — the latter is more code but strictly more correct. Split when next touched: the `dirloaded` grammar (encode + parse + its own tests) is already a fairly self-contained unit that could move to a sibling `script_dirloaded.rs` module.~~
- ~~`crates/rune-tui/src/app.rs` is 546 lines (§1.6 limit 500; was 527 immediately before this work — 16 lines above the 511 the WP4 entry above recorded, apparently grown further by an intervening WP3/WP6 merge that never added its own TODO line; not re-audited here, only the delta this work itself added is accounted for) — grown ~19 lines by the WP5 Open Tabs wiring (the `tabs: OpenTabs`/`pending_close_on_save` fields and their doc comments, the `App::new`/`open_document` initializers, and the `Pane::Tabs` stage-3 arm switching from a stub to `opentabs::handle_key`). Same deferred fix as the WP4 entry above: extracting the four-stage `handle_key`/`handle_editor_key`/`handle_db_event` trio into their own module would bring this back under budget; out of scope for WP5 itself.~~
- **RESOLVED (2026-07-28) by the same WP1 split — the generic machinery moved to `binding.rs`, `GlobalCommand`/`GLOBAL_BINDINGS` to `global.rs`; `keymap.rs` is now under 500 lines and both new modules are well under it:** `crates/rune-tui/src/keymap.rs` is 608 lines (§1.6 limit 500; was 595, already pre-existing-overage territory — see the WP2 entry above) — grown ~13 lines by WP5's `GlobalCommand::FocusTabs` + its `GLOBAL_BINDINGS` entry (`^t`), needed so the Tabs pane is reachable at all: nothing before WP5 ever set `App.focus = Pane::Tabs` in production (Go's own `FocusExplorer`/`ctrl+x` covers ONE shared filetree+tabs pane; this port's decision 7 deliberately splits them into two, which the plan's WP2/WP5 text never assigned a focus chord for — see this work's own Deviations). Same fix as the WP2 entry: extracting `KeyPattern`/`Binding`/`resolve_in`/`KeyOutcome` into their own module when next touched.
- ~~`crates/rune-tui/src/app.rs` is 555 lines (§1.6 limit 500; was 546 immediately before this work — see the WP5 entry above) — grown ~9 lines by WP7's `help_doc: Option<DocumentId>`/`help_return_to: Option<DocumentId>` fields, their doc comments, and their `App::new` initializers; the logic itself (`toggle_help`) was kept out of `app.rs` entirely, in `workspace.rs`, per the WP7 task brief's explicit "do NOT grow app.rs further" instruction — only the two fields a struct necessarily needs on `App` itself were added here. Same deferred fix as the WP4/WP5 entries above: extracting the four-stage `handle_key`/`handle_editor_key`/`handle_db_event` trio into their own module would bring this back under budget; out of scope for WP7 itself.~~
- **RESOLVED (markdown-table-rendering plan, WP3):** `crates/rune-tui/tests/tui_render.rs` was 520 lines (§1.6 limit 500) — the WP2 entry below stayed at 520, never re-growing further, since WP3's own new render-assertion tests (`table_borders_render_at_the_predicted_display_rows`, `caret_row_below_a_table_matches_wrap_to_display_of_its_wrap_row`) landed in a new sibling `crates/rune-tui/tests/tui_render_tables.rs` instead, per this exact TODO entry's own suggestion — reusing `testgrid`/`app_for`/`EDITOR_TOP_ROW` conventions duplicated locally, this crate's established per-test-file pattern (`tests/chrome.rs`/`tests/banner.rs`). `tui_render.rs` itself is untouched by WP3, still at 520 lines — still over budget, but not grown further; splitting ITS existing tests into more focused files remains open for whoever next touches it.
- `crates/rune-tui/src/app.rs` is 625 lines (§1.6 limit 500; was 586 immediately before this work — 31 lines above the 555 the WP7 entry above recorded, grown by an intervening merge that never added its own TODO line; not re-audited here, only this work's own delta is accounted for) — grown ~39 lines by the chrome-parity plan's geometry chokepoint (WP3): the `frame_width: u16` field and its doc comment, the `relayout()` method and its doc comments, and the `Msg::Resize`/`sync_view` rewrites that call it. The geometry logic itself was kept out of `app.rs` entirely, in the new `layout.rs` (WP3's own module, 175 lines) — `app.rs` gained only the one field a struct necessarily needs and the one method that bridges it to `layout::geometry`. Same deferred fix as the WP4/WP5/WP7 entries above: extracting the four-stage `handle_key`/`handle_editor_key`/`handle_db_event` trio into their own module would bring this back under budget; out of scope for the chrome-parity plan itself.
- `crates/rune-fuzz/src/generate.rs` is 558 lines (§1.6 limit 500) — unrecorded here until a later code review caught it: it crossed the 500-line ceiling to ~507 lines back at commit `85aa7c2` (WP4's Explorer `DirLoaded` generator arm) with no TODO entry added at the time, and had since grown by another ~16 lines from a review fix threading an `arb_dir_loaded_generation` strategy through the same `cluster_chrome` arm (the `Explorer::request_generation` staleness-guard fix, see the "review fixes" entries in this file's history) — already at 539 lines (this entry's recorded 523 had itself drifted, from an intervening merge that never updated it) before the markdown-table plan's WP5.S2 added a GFM table `CONTENT_SEED` and three table fragments (a row, a delimiter, an inline-alignment delimiter) to `MARKDOWN_FRAGMENTS`, growing it a further 19 lines to 558. Split when next touched: `arb_cluster`'s 11 per-cluster generator functions (`cluster_type_prose`, `cluster_navigate`, ... `cluster_chrome`) are each fairly self-contained and could move to a sibling `generate_clusters.rs` module, mirroring `script.rs`'s own suggested `dirloaded` split above.
- ~~`crates/rune-tui/src/app.rs` is 625 lines (§1.6 limit 500; was 586 immediately before this work — 31 lines above the 555 the WP7 entry above recorded, grown by an intervening merge that never added its own TODO line; not re-audited here, only this work's own delta is accounted for) — grown ~39 lines by the chrome-parity plan's geometry chokepoint (WP3): the `frame_width: u16` field and its doc comment, the `relayout()` method and its doc comments, and the `Msg::Resize`/`sync_view` rewrites that call it. The geometry logic itself was kept out of `app.rs` entirely, in the new `layout.rs` (WP3's own module, 175 lines) — `app.rs` gained only the one field a struct necessarily needs and the one method that bridges it to `layout::geometry`. Same deferred fix as the WP4/WP5/WP7 entries above: extracting the four-stage `handle_key`/`handle_editor_key`/`handle_db_event` trio into their own module would bring this back under budget; out of scope for the chrome-parity plan itself.~~
- ~~`crates/rune-fuzz/src/generate.rs` is 523 lines (§1.6 limit 500) — unrecorded here until a later code review caught it: it crossed the 500-line ceiling to ~507 lines back at commit `85aa7c2` (WP4's Explorer `DirLoaded` generator arm) with no TODO entry added at the time, and has since grown by another ~16 lines from a review fix threading an `arb_dir_loaded_generation` strategy through the same `cluster_chrome` arm (the `Explorer::request_generation` staleness-guard fix, see the "review fixes" entries in this file's history). Split when next touched: `arb_cluster`'s 11 per-cluster generator functions (`cluster_type_prose`, `cluster_navigate`, ... `cluster_chrome`) are each fairly self-contained and could move to a sibling `generate_clusters.rs` module, mirroring `script.rs`'s own suggested `dirloaded` split above.~~
- `crates/rune-tui/src/explorer.rs` is 522 lines (§1.6 limit 500) — pre-existing overage that had no entry here at all until the space-leader plan's WP1 audit found it (2026-07-28); not grown by that work. Split when next touched: the `EXPLORER_BINDINGS` table + `handle_key` are a self-contained unit that could move to a sibling module, mirroring the `keymap.rs` → `binding.rs`/`global.rs` split above.
- ~~`crates/rune-fuzz/src/driver.rs` is 543 lines (§1.6 limit 500; was 494, already zero headroom before this work) — grown ~49 lines by the `UNDO-TOTAL` harness fix (fuzz catch `undo-total-8c5284f3`: the end-of-session `⌘Z` drive pressed undo into a non-`Editor`-focused pane, which correctly ignores it). The new logic was already extracted into its own documented helper, `restore_editor_focus` (dismisses a modal via `Escape`, then focuses the editor via `^E`, both through `step_and_check` so every invariant still runs over the presses) — but even the bare code addition (the helper's signature + body, the one-line `Pane` import, and the three-line `if` reformat at the drive's entry point) is ~28 lines, already past the 6-line headroom that existed at 494; no further extraction was available without moving the helper to its own module and making `State`/`Outcome`/`step_and_check`/`key_step` `pub(crate)` across a module boundary, which is a bigger restructure than this fix's scope. Split when next touched: hoist `State`/`Outcome`/`step_and_check`/`run_update_catching_panic`/`downcast_panic` into a sibling `driver_step.rs` module, leaving `driver.rs` itself as just `run` + the end-of-session drive + `key_step`/`restore_editor_focus`.~~
- ~~`crates/rune-tui/src/app.rs` is 668 lines (§1.6 limit 500; was 625 immediately before this work — see the chrome-parity entry above) — grown ~43 lines by the space-leader plan's WP5 wiring: the `space_probe: Box<dyn keystate::SpaceProbe>` and `speculative_space: Option<DocumentId>` fields (with their doc comments) and their two `App::new` initializers, the Stage 1.5 held-space-leader completion inserted into `handle_key` between modal capture and the global table, and the one-line arming of `speculative_space` in `handle_editor_key`'s printable fallthrough. Same deferred fix as every other `app.rs` entry above: extracting the four-stage `handle_key`/`handle_editor_key`/`handle_db_event` trio into their own module would bring this back under budget; out of scope for this plan itself (its own WP1 split `keymap.rs`, not `app.rs`).~~
- `crates/rune-tui/src/commands/edit.rs` is 828 lines (§1.6 limit 500; was 695 before this work — see the entry above) — grown ~133 lines by the space-leader plan's WP4 `retract_space` (the bespoke single-cursor, selection-safe space-retraction edit decision 5 requires instead of `delete_left`, which is selection-first and would delete the user's selected text) plus its six unit tests, including the two data-loss regression guards asserting an active selection and a multi-cursor set each survive the chord intact. Those two guards were originally written vacuous — the selection fixture put the caret where the byte to its left was not a space, so `retract_space`'s byte guard returned first and the selection guard was never reached; both tests passed with the guard deleted. Caught by mutation testing (delete the guard, confirm the tests fail) and fixed; the fixtures now sit immediately right of a space, and the tests carry a comment recording the trap. Split by command family when next touched, as the entry above already recommends.
- **RESOLVED (markdown-table-rendering plan, WP4) — exactly the split this entry itself suggested:** `crates/rune-md/src/emit/walk.rs` was 592 lines (§1.6 limit 500; was already 540 — pre-existing overage, no TODO entry recorded when it first crossed the ceiling during the markdown-table plan's WP1 scaffold — immediately before that work) — grown ~52 lines by the markdown-table plan's WP2.S3/S7: every `emit_block`/`emit_inline` signature now threads one `&mut EmitOut` bundle instead of three loose out-params, and the temporary WP1 `Block::Table` scaffold (raw verbatim passthrough regardless of reveal state) is replaced by the real Grid-layout `emit_table` (render every row's cells, compute column widths, then tile each source line's Grid/separator row via `table::row_spans`). WP4's own Wrapped/Pivoted layout dispatch grew `emit_table` further still (592 -> 724 lines) before this fix — `emit_table` and its supporting match arms moved to the new sibling `crates/rune-md/src/emit/table.rs` (257 lines), mirroring `table::{render,layout}`'s own split; `walk.rs` is now 476 lines, under the §1.6 limit.
- **RESOLVED (2026-07-28) during integration of the same work that caused it:** `crates/rune-syntax/src/wrap/mod.rs` briefly reached 539 lines (§1.6 limit 500; was 477) when the markdown-table plan's WP2.S8 added `TableSegInfo` and the wrap pass's table branch. Both moved to a sibling `wrap/table.rs` (61 lines), bringing `wrap/mod.rs` back to 489. The seam is real rather than cosmetic: "a pre-laid-out line bypasses greedy breaking entirely" is a different concern from the greedy breaker itself.
- ~~`crates/rune-tui/src/render.rs` is 503 lines (§1.6 limit 500; was 493 immediately before this work, already 7 lines from the ceiling) — grown ~10 net lines by the markdown-table plan's WP3 display-space rewiring: `segment_cells`/`segment_geometry`/`segment_cells_with` now take a plain `&[SyntaxSpan]` instead of a whole `WrapSegment` (so a `DisplayRow`'s synthesised border spans, which have no backing `WrapSegment`, can be walked identically), `build_rows` iterates `view.display.rows()` instead of `view.wrap.segments()`, and `apply_cursor_overlays` converts the cursor's wrap row through `DisplaySnapshot::wrap_to_display` before indexing the DISPLAY-space `rows` — each of these needed a doc-comment explaining the wrap-vs-display split, which is most of the added lines. Split when next touched: `push_grapheme_cells`/`segment_cells_with`/`segment_cells`/`segment_geometry` (the per-cell width/grapheme walk) are already a fairly self-contained unit that could move to a sibling `render_cells.rs`, leaving `render.rs` itself as `build_rows`/`apply_cursor_overlays`/`draw`/`blit`.~~

## rust port — RESOLVED: `CGEventSourceKeyState` over SSH is unverified, and stalls without a window-server session (recorded 2026-07-28)

**Status: RESOLVED (2026-07-28).** The SSH gate below was voided by plan amendment — not run, not required — once the root cause it was meant to detect was fixed at the source instead: `keystate::leader_available()` (`crates/rune-tui/src/keystate.rs`) now calls `CGSessionCopyCurrentDictionary()` FIRST and short-circuits `false` when it's NULL, so `CGEventSourceKeyState` is never reached without a window-server session — the exact stall this entry documents can no longer occur, detection replaced by prevention. `rune-cli/src/main.rs` also now primes `leader_available()`'s `OnceLock` at startup (immediately after installing `HidSpaceProbe`), closing concern (1) below directly: the one-time cost lands at startup, off the keystroke path, never on the user's first `space+x`/`e`/`t` press. WP4 (`retract_space`), WP5 (leader wiring) and WP6 (contextual footer) subsequently completed and all passed their Done-when gates. Original text preserved below for the record.

The held-space-leader plan's WP3 gate requires proving the `CGEventSourceKeyState` probe returns promptly with no window-server session:

```sh
ssh localhost "timeout 30 cargo test -p rune-tui --test keystate_cost -- --ignored --exact keystate_query_is_cheap"
```

**This gate could not be run: Remote Login is disabled on this machine** (`nc -z localhost 22` fails, `ssh localhost` gets "Connection refused"; enabling sshd needs admin rights and changes the machine's security posture, so it was not done unasked). Per the plan's explicit stop condition, work stopped at the end of WP3 — **WP4 (`retract_space`), WP5 (leader wiring) and WP6 (contextual footer) were not started.**

Measured evidence that the underlying risk is real, not hypothetical (`HidSpaceProbe::space_is_down`, debug build):

- Normal GUI session: **first call 13.6 ms** (one-time bootstrap), then **~17 ns/call** — comfortably under the plan's 10 µs/call threshold. `keystate_query_is_cheap` and `keystate_query_does_not_prompt_for_permissions` both pass here.
- Inside a restricted sandbox with the mach lookup to the window server denied: **the first call never returned** (killed at 45 s, and again at 90 s under `cargo test`). It does not fail fast and it does not return `false` — it blocks.

So CGEventSourceKeyState over SSH / headless is not merely unverified; the one adjacent environment that could be tested shows an unbounded stall on the first call. Two consequences to resolve before WP4-WP6 proceed:

1. `keystate::leader_available()` caches its answer in a `OnceLock`, but nothing calls it at startup — it is reached lazily from the first `space_is_down()`, i.e. on the user's first `x`/`e`/`t` keystroke. If that first call blocks, the editor hangs on a keystroke. It must be primed during startup (off the key path) *and* given a bound, or the leader must be gated off when no window-server session is present.
2. The one-shot check cannot distinguish "no session" from "space is up" — `CGEventSourceKeyState` returns a bare `bool` with no error channel. A session probe that does have an error channel (e.g. checking for an Aqua session before installing `HidSpaceProbe` in `rune-cli::main`) is the likely fix.

Landed regardless, since WP1-WP3 are self-contained and green: the `keymap.rs` -> `binding.rs`/`global.rs` split, the `^D^D` quit chord, and the `keystate` module itself (built, linked, clippy-clean, both non-SSH FFI tests passing). The leader is inert — every `App` gets `NullProbe` — until WP5 wires it.

- `crates/rune-tui/src/rename.rs` is 612 lines the day it lands (§1.6 limit 500) — a NEW file over budget, which is worth stating plainly rather than burying: roughly half of it is doc comment (the machine's states and, more importantly, the three states that deliberately do NOT exist, plus the failure-atomicity reasoning §1.4.10 turns on), and the code itself is one state enum, `begin`, one `apply_outcome` match over four outcomes, two `Cmd` factories and five small hooks. The available split is real but not obviously an improvement today: the two no-store `Cmd` factories (`rename_cmd`/`create_cmd`) plus the draft-create route (`bind_new`) could move to a sibling `rename_create.rs`, leaving `rename.rs` as the state machine proper. Do that when this next grows — in particular when per-doc recovery hydration (see below) makes the `db: None` branches disappear, which should shrink this file on its own.
- ~~`crates/rune-tui/src/app.rs` is 630 lines (§1.6 limit 500; was 586 immediately before this work) — grown ~44 lines by the rename wiring: the `title`/`rename`/`next_rename_gen` fields with their doc comments and initializers, the `Pane::Title` stage-3 arm, the `Msg::RenameDone` dispatch arm, the `OpOutcome::Rename` `handle_db_event` arm, and the `at_buffer_top` helper behind the Up-at-editor-top gesture. Same deferred fix the WP4/WP5/WP7 entries above all name and none of them performed: extract the four-stage `handle_key`/`handle_editor_key`/`handle_db_event` trio into their own module. This is now the fifth consecutive work package to defer it, which is itself the signal — it should be done before `app.rs` is touched again, not after.~~
- `crates/rune-tui/src/keymap.rs` is 619 lines (§1.6 limit 500; was 608) — grown ~11 lines by `GlobalCommand::FocusTitle` + its `^r` `GLOBAL_BINDINGS` entry. Same fix as the WP2/WP5 entries above.
- `crates/rune-tui/src/explorer.rs` is 553 lines (§1.6 limit 500; was 527) — grown ~26 lines by `refresh_for`, the post-rename re-listing (a `pub(crate)` sibling of the private `request_dir`, using `DirCause::Refresh` so a rename preserves the user's selection instead of snapping to the top — a rename is not a navigation). `DirCause::Refresh` had until now been a shape with no production caller; this is its first one.
- ~~`crates/rune-tui/src/app.rs` is 720 lines (§1.6 limit 500; was 712 immediately before this work — an intervening merge grew it from the 630 the rename entry above recorded, with no TODO entry added at the time; not re-audited here, only this work's own delta is accounted for) — grown 8 lines by the sequence-capable-keymap plan's WP6.S8 `binding_set: crate::keymap::BindingSet` field (its doc comment and `App::new` initializer) — the one piece of that package's design that has to live on `App` itself; `app::handle_editor_key` does not read it yet (full vim modal editing is out of scope). Same deferred fix every `app.rs` entry above names: extract the four-stage `handle_key`/`handle_editor_key`/`handle_db_event` trio into their own module; this is now the sixth consecutive work package to defer it.~~
- `crates/rune-tui/src/explorer.rs` is 554 lines (§1.6 limit 500; was 548 immediately before this work) — grown 6 net lines by the WP6.S1 `Binding<C>` shape change (`key: KeyPattern` → `keys: &'static [KeyPattern]` plus a new `when: &'static str` field): `EXPLORER_BINDINGS`'s six literals each gained one line for the new field. No logic changed. Same split suggestion as the entry above: the `EXPLORER_BINDINGS` table + `handle_key` are a self-contained unit that could move to a sibling module.
- `crates/rune-tui/src/app.rs` is 729 lines (§1.6 limit 500; was 720 immediately before this work — see the WP6.S8 entry above) — grown 9 lines by the editor-MVP plan's WP9 command dispatch: the `edit_lines`/`multi` entries in the `commands::{...}` import, and nine new `match command` arms (`DeleteWordLeft`/`Right`, `DeleteLine`, `Indent`/`Outdent` retargeted to `edit_lines`, `MoveLineUp`/`Down`, `CloneLineUp`/`Down`, `AddCursorAbove`/`Below`) each a single dispatch line. Same deferred fix every `app.rs` entry above names: extract the four-stage `handle_key`/`handle_editor_key`/`handle_db_event` trio into their own module; this is now the seventh consecutive work package to defer it.
- `crates/rune-tui/src/app.rs` is 801 lines (§1.6 limit 500; was 779 immediately before this work, per this task's own brief — grown further from the 729 the WP9 entry above recorded by intervening merges that never added their own TODO line; not re-audited here, only this work's own delta is accounted for) — grown ~22 lines by WP6's per-doc recovery hydration: the `db_load_versions: HashMap<u64, u64>` field and its doc comment, the `App::new` initializer, the `OpOutcome::Load` `handle_db_event` arm, and one line each clearing `db_load_versions` in the `Err`/`Fatal` arms alongside the pre-existing `db_ops` clears. The task brief explicitly scoped this file to "add ONLY the one match arm" — the field declaration/initializer were unavoidable (the arm reads `app.db_load_versions`) but are the minimum a struct field requires. Same deferred fix every `app.rs` entry above names: extract the four-stage `handle_key`/`handle_editor_key`/`handle_db_event` trio into their own module; this is now the eighth consecutive work package to defer it.

## rust port — `crates/rune-tui/src/db.rs` crosses the §1.6 500-line budget (recorded 2026-07-28, WP6)

`crates/rune-tui/src/db.rs` grew from 484 to 569 lines adding WP6's per-doc recovery hydration: `load_document` (the `Store::load` enqueue, mirroring `append_edit`/`move_undo_pos`) and `handle_load_ack` (the ack reaction — installs `DocDb`, and the version-guarded buffer adopt). This is the first time this file has crossed the budget. `handle_load_ack` is the larger of the two new functions (the version-guard branch and its doc comment); splitting `db.rs` into an enqueue-side module and an ack-reaction-side module (mirroring `save.rs`'s own split from `app.rs`) would bring it back under budget — deferred, out of scope for WP6 itself.

## rust port — WP5 "follow a link" §1.6 overages (recorded 2026-07-28)

- `crates/rune-tui/src/document.rs` is 502 lines (§1.6 limit 500; was 494 immediately before this work, already 6 lines from the ceiling) — grown 8 lines by the `pub catalogue: Vec<rune_nav::Ref>` field (its doc comment, the `App::new`-mirroring `Document::new` initializer, and the one-line rebuild call in `Document::view`). This is the file's first crossing of the §1.6 ceiling. The task brief scoped this file to exactly this field + the one rebuild call; no further reduction was available without moving `Viewport`/`ScrollMode` (roughly the first third of the file, already a self-contained unit with its own tests) to a sibling module. Split when next touched.
- `crates/rune-tui/src/keymap.rs` is 584 lines (§1.6 limit 500; was 573 immediately before this work, already pre-existing-overage territory — see the many entries above tracking this file's chronic overage since its first crossing) — grown 11 lines by the WP5 `Command::FollowLink` variant (its doc comment) and the two `KeyCode::Enter` resolver arms (⌘Enter/^Enter) plus their doc comment. Same fix every entry above names: extract `KeyPattern`/`Binding`/`resolve_in`/`KeyOutcome`-adjacent machinery, or `Command`/`resolve` itself, into its own module.
- `crates/rune-tui/src/app.rs` is 820 lines (§1.6 limit 500; was 818 per this task's own brief) — grown 2 lines by the WP5 `Command::FollowLink` dispatch arm and its one `use crate::navigate;` import, per the task brief's explicit "add ONLY a `Command::FollowLink` dispatch arm" instruction. Same deferred fix every `app.rs` entry above names: extract the four-stage `handle_key`/`handle_editor_key`/`handle_db_event` trio into their own module; this is now the ninth consecutive work package to defer it.
- ~~`crates/rune-tui/src/app.rs` is 729 lines (§1.6 limit 500; was 720 immediately before this work — see the WP6.S8 entry above) — grown 9 lines by the editor-MVP plan's WP9 command dispatch: the `edit_lines`/`multi` entries in the `commands::{...}` import, and nine new `match command` arms (`DeleteWordLeft`/`Right`, `DeleteLine`, `Indent`/`Outdent` retargeted to `edit_lines`, `MoveLineUp`/`Down`, `CloneLineUp`/`Down`, `AddCursorAbove`/`Below`) each a single dispatch line. Same deferred fix every `app.rs` entry above names: extract the four-stage `handle_key`/`handle_editor_key`/`handle_db_event` trio into their own module; this is now the seventh consecutive work package to defer it.~~

## rust port — accepted deviation: save-failure messages stay on the footer, not the Banner (recorded 2026-07-27, WP3.S4 vs WP3's original plan)

`save.rs`/`app.rs` route a failed (or un-attempted) save through `app.set_status(..., StatusSource::SaveError)` — the footer's `Mode::SaveError` display (`footer.rs`) — rather than through `banner::report_error`, even though WP3.S4's plan text named `report_error` as "the one chokepoint every error report funnels through." This was the WP3 worker's own documented deviation at the time (plan WP3.S4), re-surfaced and RATIFIED (not flagged as a defect) by a later code review: save-failure messages are short, recoverable one-liners ("save failed: disk full", "no file to save — rune was opened without a path") that the user dismisses just by continuing to type or by fixing the underlying problem and pressing ⌘S again — exactly the footer's job. The Banner is reserved for multi-line, must-be-acknowledged error context (a read failure's full message, an unexpected exception) where scrolling/copy-via-OSC-52 actually matter. Routing SaveError through the Banner would interpose a full-screen modal on every transient save hiccup, which is a worse UX than the footer's own priority-ranked, self-clearing display. No action needed — this entry exists only so the deviation from the plan's literal chokepoint wording reads as a considered decision, not an unreviewed gap.

## rust port — per-doc recovery hydration for explorer-opened documents (recorded 2026-07-27, WP4.S5, Assumption A1; closed 2026-07-28, WP6)

**Status:** closed — Explorer-opened documents now hydrate through the same app-wide `Store`, non-blocking and ack-driven.

`workspace::open_path` now calls `db::load_document` right after `bind_path`, enqueuing a `Store::load` for the newly opened document exactly like the pre-existing `append_edit`/`move_undo_pos` enqueue sites; the ack lands as an ordinary `Msg::Db` and is routed by `app::handle_db_event`'s `Load` arm to `db::handle_load_ack`, which installs `Document::db` from the ack's `LoadResult`.

**Residual gap (accepted, not a defect):** `Load` is asynchronous, so the user can keep typing during the round trip. `db::load_document` records the document's `buffer.version()` at the moment the op is enqueued (`App::db_load_versions`); `db::handle_load_ack` adopts the ack's `recovered` content into the buffer ONLY if that version is still unchanged at ack time. When the user typed in the meantime, `DocDb` is still installed (this document's recovery journal is real and should be used going forward), but the buffer bytes are left exactly as typed — this session's CAS baseline simply anchors from the disk content `open_path` already read, the same as a load with no divergence would. The window is bounded to one `Store::load` round trip per newly opened document and never risks a keystroke, only (rarely) a crash-recovered draft from a PRIOR session not being auto-merged into a document that was itself edited before its own hydration finished.

## rust port — rename's displaced-bytes attribution is a latent merge-ancestor risk (recorded 2026-07-28, WP11.S5, plan Assumption A3)

**Status:** open; disclosed risk to re-examine, not a defect to fix now.

- **Symptom:** none reproduced — this records a latent risk in the merge-ancestor derivation, not an observed failure.
- **Root cause:** `rune-db`'s rename path files a replaced file's displaced bytes under the **renaming** document's `doc_id` — its own test asserts `displaced.doc_id == f.ds.doc_id, "captured under OUR doc"` — and blanks any pre-existing `documents` row's path for the replaced file. CONSTITUTION §12 makes the `observations` stream the source `newestObservation`/`ancestorAt` read from — the 3-way-merge baseline. Filing a foreign file's hash and stat under the wrong document's observation stream is therefore a latent wrong-ancestor risk, and a wrong ancestor silently discards a real change (§0.1 Catastrophic).
- **Where:** `rune-db/src/rename.rs`, on `rr` — `worktree-kind-inventing-marshmallow` was fast-forward merged in at `74dc1ad`. (This entry was first drafted while that work was still unmerged; the risk is now live on the mainline, not pending.)
- **Fix design:** none yet — accepted as shipped (plan Assumption A3), and deliberately not re-litigated during the editor MVP.
- **Next step:** actionable now that the branch has landed. Audit whether a rename's displaced-bytes observation can ever become the merge ancestor for a document other than the renaming one; if so, scope `ancestorAt`/`newestObservation` to exclude cross-doc-attributed observations. A regression test would bind a document, rename a second file over it, then assert the first document's ancestor is unchanged.

## rust port — the 100ms display-pipeline budget permits a visibly laggy keystroke (recorded 2026-07-28, WP11.S6)

**Status:** RESOLVED for the concrete regression (2026-07-29, keystroke-latency plan WP16) — the disclosed §5.3 tension this entry recorded is what WP16 fixed; the original 100 ms full-pipeline budget test itself is unchanged (it still measures a worst-case COLD run, by design) but the synchronous per-keystroke path it worried about no longer pays that cost on an ordinary keystroke.

- **Symptom (as filed):** none observed — a budget is not a measurement; this recorded what the budget as written permitted, not an observed regression. **It later became one**: keystroke latency visibly regressed after the tree-sitter (`rune-ts`) merge, tracing to exactly the synchronous-pipeline-cost tension this entry had already flagged, compounded by three more costs the merge introduced on the same path.
- **Root cause (as filed):** `full_pipeline_5k_under_100ms` (`crates/rune-md/tests/perf_guard.rs`) permits a 100 ms full display-pipeline run on a 5,000-line document, and `App::sync_view` runs that pipeline synchronously — no async offload — from the runtime's blocking message loop before every frame is drawn. §5.3 requires `update` to stay non-blocking; a 100 ms stall on every keystroke sits uneasily with that.
- **What WP16 actually found and fixed:** `DocMachine::snapshot` (and `Document::view`'s catalogue rebuild) re-ran the full emit + wrap + `expand_tables` pipeline UNCONDITIONALLY on every `view()` call, even when nothing relevant had changed since the last call in the same message batch (WP16.S1: memoized on a real dirty flag). `highlight::schedule_highlight` cloned the entire buffer to a `String` on the UI thread BEFORE checking whether a highlight was even needed (WP16.S2: gates hoisted above the clone). `rune-ts::highlight` re-parsed the whole document from scratch on every call, never incrementally (WP16.S3: `rune_ts::Reparser` retains the tree-sitter tree per document). The highlight overlay scanned every stored span per frame regardless of the visible window (WP16.S4: binary search via `partition_point`). Every journal-mutating message spawned a fresh OS thread that slept 2s for the snapshot-autosave debounce (WP16.S5: one rearmable timer thread per app, `App::snapshot_timer`).
- **Verification:** `crates/rune-tui/tests/perf_guard.rs`'s `keystroke_view_cost_under_budget_on_a_5k_line_code_document` (run via `make perf-guard`, alongside the original `rune-md` guard) asserts the SYNCHRONOUS per-keystroke cost (one `app::update` + `App::sync_view`, no spawned `Cmd` ever run — matching how the real runtime only pays a `Cmd`'s own cost off-thread) stays under a stated budget, averaged over 100 keystrokes into the same shape of large document this entry's original guard used.
- **Still open, deliberately not addressed by WP16:** the underlying §5.3 tension itself — `sync_view` is still a synchronous call inside the blocking loop, and the ORIGINAL `full_pipeline_5k_under_100ms` 100 ms budget is unchanged (a genuinely COLD sync — the first `view()` after opening a huge document, or after an edit large enough to invalidate every memo — still pays close to that full cost, by design; WP16 only removes the cost an ORDINARY keystroke pays needlessly). Moving the recompute itself off the synchronous path (incremental recompute deeper in `rune-md`, or an async worker) remains a larger redesign, out of scope here.

## ~~rust port — Rust corrupts ZWJ/skin-tone emoji grapheme clusters (recorded 2026-07-28, glyph-grid parity plan WP1)~~

~~Discovered by the new `parity-grid` gate's `scripts/parity/fixtures/emoji.md` fixture (excluded from the gate itself — see `scripts/parity/README.md`'s "Known divergences" — since this is a real defect to fix, not a divergence to accept). A ZWJ family emoji (`👨‍👩‍👧‍👦`, 7 codepoints joined by U+200D) and skin-tone-modified emoji (`👋🏽`, `👍🏿`) render visibly corrupted in the Rust TUI's `tmux capture-pane` output: stray joiner/emoji fragments appear out of order, and extra spaces are inserted mid-sequence — e.g. `family 👨‍👩   ‍👧   ‍👦    sequence` instead of `family 👨‍👩‍👧‍👦 sequence`, with a stray `‍👦` fragment even leaking earlier into the same line. Go renders both correctly. Root cause not yet investigated; likely a grapheme-cluster segmentation or cell-width gap somewhere in the Rust cell-building/render pipeline for multi-codepoint ZWJ sequences (`rune-md`'s `wrap`/emit path, or `rune-tui`'s `render::segment_cells`) that treats each codepoint as its own cell/cluster instead of the whole joined sequence. Reproduce: `PARITY_SCENARIO=01-open-file scripts/parity/capture.sh rust 01-open-file emoji.md` then inspect `.scratch/parity/out/rust.txt`.~~

**RESOLVED (2026-07-28, defect-fix session).** Root cause: the pipeline built ONE `Cell` per `char`, not per grapheme cluster — a ZWJ sequence's 7 codepoints (or a skin-tone emoji's 2) each got their own buffer cell. `ratatui::buffer::Buffer`'s own diffing (`BufferDiff`, ratatui-core 0.1.2) treats any cell whose `cell_width()` is `> 1` as covering the next `cell_width() - 1` columns too and silently skips re-examining them ("we're assuming buffers are well-formed, that is no double-width cell is followed by a non-blank cell" — its own doc comment); a REAL codepoint sitting in exactly that "covered" column never reached the real terminal, and every later column on the row shifted — the observed reordering/stray-fragment corruption. Fix, in two parts:
- `crates/rune-tui/src/render.rs`: `Cell.ch: char` → `Cell.text: String`; `push_char_cells` → `push_grapheme_cells`, walking `unicode_segmentation::graphemes(text, true)` instead of `chars()` (both in `segment_cells`'s `Substituted`/`Identical` branches); `blit` now resets (`ratatui::buffer::Cell::reset`) every continuation column a wide `Cell` covers, matching `Buffer::set_stringn`'s own convention (the low-level `cell_mut` writes this code uses don't do that automatically).
- `crates/rune-md/src/wrap/mod.rs`: added `grapheme_width`/`grapheme_width_with_tab`, the grapheme-cluster counterpart to `control_aware_width`/`rune_width_with_tab` — used by `wrap_line`'s greedy line-breaking (also switched to iterate graphemes, not chars) and by `query.rs`'s `visual_col`/`byte_col_from_visual`, so wrap/render/caret math all agree. Getting the WIDTH FORMULA right took two attempts, both verified against a real `tmux capture-pane` (not just the `TestBackend`-only unit tests, which never exercised the real diff/print path where this actually manifests): summing each rune's width (even unclamped) reserved MORE columns than tmux's own real cluster-width consumption, leaving a visible gap of blank columns before whatever followed on the row — confirmed empirically by forcing the cluster width to a constant `2` and observing the gap vanish. The correct, general formula is the MAX of each rune's raw `unicode_width::UnicodeWidthChar::width` (never the SUM, never `control_aware_width`'s 1-clamp) — this also correctly gives width 1 to a base-char-plus-combining-mark cluster, not just width 2 to an emoji cluster.
- Regression tests: `crates/rune-tui/tests/tui_render.rs`'s `zwj_family_emoji_renders_as_one_cell_and_buffer_bytes_round_trip`, `skin_tone_modifier_emoji_renders_as_one_cell_and_buffer_bytes_round_trip`, `wide_cell_leaves_a_blank_continuation_column_in_the_real_backend`.
- `scripts/parity/fixtures/emoji.md` now renders byte-for-byte identically to Go except for the pre-existing, separately-tracked "list-marker gap" (Go never conceals plain bullet markers; Rust conceals all of them, including this fixture's own `- 🚀 launch item` etc.) — confirmed via a real `make parity-grid` capture. The fixture stays excluded from the gate (`scripts/parity/grid.sh`'s `excluded_reason`) for that unrelated, already-accepted reason, not for ZWJ corruption.

## ~~rust port — Rust renders no checkbox glyph for GFM task list items~~

**RESOLVED (2026-07-28, defect-fix session).** `scripts/parity/fixtures/tasks.md` (glyph-grid parity harness) showed Rust rendering neither the raw `[ ]`/`[x]` brackets nor a checkbox glyph for a concealed (cursor-elsewhere) GFM task item — the whole checkbox marker vanished, where Go substitutes `☐`/`☑`. Root cause: `rune-md/src/emit/walk.rs`'s `emit_list_item` `hide_range`d a task item's WHOLE marker (the `"- [ ] "` prefix, including the checkbox itself) when concealed, with nothing substituted in its place — the `StyleId::TaskMarker` style existed (used by the revealed/raw path) but nothing ever produced a `Substituted` span for it. Fix: `emit_list_item`'s concealed branch now hides only the `"- "`/`"1. "` prefix before the checkbox (same as a plain marker) and calls a new `push_task_checkbox`, which substitutes the item's own `task: ByteRange` (always exactly the 3 ASCII bytes `"[ ]"`/`"[x]"`/`"[X]"`, per `ListItemM::task`'s docs) with `☐` (U+2610) or `☑` (U+2611) — each also exactly 3 UTF-8 bytes, so the substitution is byte-length-neutral by construction and needs no extra `SyntaxSnapshot` hidden-range bookkeeping (that coordinate model only accounts for FULLY hidden byte ranges, not length-changing substitutions). The trailing space(s) between the checkbox and the item's own text are deliberately left unclaimed for `fill_gaps` to supply verbatim, so 0/1/N trailing spaces round-trip exactly instead of a hardcoded second space. Updated `rune-md/tests/conceal_roundtrip.rs`'s `tasklist_marker_conceals_off_cursor_line` to assert the new `"☑ task"` output. `scripts/parity/fixtures/tasks.md` now matches Go byte-for-byte except for the same pre-existing "list-marker gap" as `lists.md` (its own `- [ ]`/`- [x]` prefixes and its one plain `- a plain bullet, not a task` line) — confirmed via a real `make parity-grid` capture; stays excluded from the gate for that unrelated reason.

## rust port — accepted deviation: word motion classifies Unicode letters as `Word`, not ASCII-only like Go (recorded 2026-07-28, editor-MVP plan WP9.S1)

**Status:** deliberate divergence, not a defect — recorded per plan WP9.S1's explicit instruction.

Go's `commands_nav.go:getClass` classifies `[A-Za-z0-9_]` as `Word` and every other rune — including every non-ASCII letter (Cyrillic, Greek, CJK ideographs, combining marks, …) — as `Other` (punctuation-like). Since `wordLeftOffset`/`wordRightOffset` stop at every class change, `⌥←`/`⌥→` in Go stop at EVERY individual non-ASCII character instead of the actual word boundary — visibly broken for Cyrillic/Greek/etc. text. `crates/rune-tui/src/commands/nav.rs`'s `char_class` intentionally does NOT port this: it classifies a rune as `Word` when `unicode-segmentation`'s UAX #29 word-boundary algorithm (probed via `is_word_forming`) would fuse it into a word alongside ordinary letters, so `⌥←`/`⌥→` behave correctly over any script. Whitespace is likewise generalized from Go's ASCII `' '`/`'\t'`/`'\n'`/`'\r'` check to `char::is_whitespace()`. This is a fix, not a port — ported faithfully it would just carry Go's bug forward. No action needed; recorded so the Rust and Go behaviors' divergence here reads as considered, not an unreviewed gap.

## rust port — `assert.sh`'s rust-side breadcrumb check fails deterministically, not from a settle race (recorded 2026-07-28, table-rendering plan WP0.S4; corrects a 2026-07-28 glyph-grid-parity-plan-WP1 entry of the same name)

**Status: open — not fixable inside the parity-harness package (`scripts/parity/`); needs a decision.**

The prior entry here (glyph-grid parity plan WP1) diagnosed this as `rust.settle` (`╭Files`) resolving before the bottom-border breadcrumb's own async repaint landed, and proposed waiting for the breadcrumb's rendered text instead. That fix was implemented (table-rendering plan WP0.S1/S2: `capture.sh` expands a `{{FIXTURE}}` token, `rust.settle` was set to `{{FIXTURE}} *──╯`) and empirically falsified: **the breadcrumb does not render late — it does not render at all**, on every run, not intermittently.

Root cause: `crate::breadcrumb::overlay` (`crates/rune-tui/src/breadcrumb.rs`) has no dependency on `App`'s workspace root or on `Msg::DirLoaded` — it reads only `app.active_doc().file_path` and renders every `Normal` path component (the already-tracked "Breadcrumb path relativization" divergence, `scripts/parity/README.md`'s "Known divergences"). With the Explorer pane open (this scenario's `C-b`), the centre pane narrows to ~96-98 columns (`PARITY_COLS=120`), and the harness's own workspace path — `<repo>/.scratch/parity/run/rust/parityws/<fixture>`, 10 `Normal` components deep whether `<repo>` is a worktree under `/tmp` or the main checkout under `/Users/…` — makes even the most-truncated crumb (one leaf part plus the ellipsis) exceed `overlay`'s own `bc + 7 > block.width` bail-out. The result is a bare border with no text, permanently, not a slow frame. Verified via direct `tmux capture-pane` polling at 20ms/2s/5s after the key lands — identical blank border throughout — and reproduced 3/3 back-to-back `make parity` runs.

Consequence: `scripts/parity/assert.sh`'s "rust bottom content row (line 33) ends 'sample.md ──╯'" check FAILs on every run (7 PASS / 1 FAIL), so `make parity`/`make parity-assert` exit non-zero unconditionally — independent of any settle-predicate choice, since no predicate can wait for text that structurally never appears. `rust.settle` was corrected to the fixture-independent `Enter open` (a fragment of the Explorer-focused footer) so `capture.sh` itself no longer hangs/races; see `scripts/parity/README.md`'s scenario-file section. `make parity-grid`'s own gate is unaffected — `grid_diff.py` crops to rows 2..(ROWS-2), never the bottom border row.

Not fixed here: a real fix needs either (a) implementing breadcrumb workspace-root relativization in Rust (the divergence itself, a real feature, not a parity-harness change), (b) widening the harness's pinned `PARITY_COLS`, or (c) shortening the harness's own workspace-path nesting — all out of scope for `scripts/parity/`'s owned files. Needs a decision before `make parity`/`make parity-assert` can be treated as a passing gate again.

## rust port — PRE-EXISTING: `CUR-BOUNDS` fires on multicursor + page-down + CJK (recorded 2026-07-28, markdown-table plan WP5)

Surfaced by the markdown-table plan's new fuzz table seeds, which made the
session generator explore further — but **not caused by that work**. Verified by
replaying the script below at `429fb2e` (the commit the table plan branched
from, before any table code existed): it fails there byte-identically, and the
fixture contains no table at all.

```
content # Title\n\n- item one\n- item two\n\n> a quote\n\n```rust\nfn main() {}\n```\n\n[a link](https://example.com)\n
key pagedown ----
type \u{4f60}\u{597d}\u{4e16}\u{754c}\u{ff0c}\u{4e16}\u{754c}\u{4f60}\u{597d}
key up ----
key down -a-u
key char:A ----
key char:  ----
```

Violation: `CUR-BOUNDS: cursor id=2 position=100 anchor=100 content.len()=126`.
A secondary cursor (added by `alt+down`) is left at a position that is not a
valid char boundary of the post-edit content after CJK text is typed on a
page-down-scrolled view.

**Not committed to `repros/` while red** (the standing convention: a repro
belongs there only in the same commit as its fix, since `replay.rs`'s contract
is "every checked-in script replays clean"). The script above is the verbatim
copy. Fix: clamp/re-validate every secondary cursor to a char boundary at the
edit-commit chokepoint, not just the primary. Out of scope for the table plan.

## rust port — Tables: deliberately left open (recorded 2026-07-28, markdown-table plan WP6.S7)

**Status:** accepted, not fixed — each item below was a considered decision in the plan, not an oversight.

- **Assumption A1 — grid-fit threshold divergence.** Rust's Grid-fit test uses the true
  rendered row width `Σw + 3n + 1`; Go's `computeMinGridWidth` uses `Σw + 4n − 1`
  (`markdown_table_layout.go:72-81`), which is Go's own bug (also recorded in
  `golang/TODO.md`) — the two formulas agree only at exactly 2 columns. At other column
  counts, a table whose width sits within `n − 2` of the threshold can make the two
  implementations pick different layouts (Grid vs. Wrapped/Pivoted) at the same terminal
  width. `scripts/parity/fixtures/tables-narrow.md` is sized well past both formulas'
  thresholds specifically to avoid exercising this gap in the gated corpus; a user
  resizing a real terminal to exactly that straddle width would still see it.
- **Assumption A2 — hard-break-by-display-width vs. hard-break-by-rune-count.** Rust
  hard-breaks an over-long word (no whitespace to wrap at) by accumulated display width;
  Go's `hardBreakWord` (`markdown_table_layout.go:249-270`) breaks by rune count, so a
  CJK-heavy over-long word overflows Go's column by up to 2x. Every gated fixture avoids
  words longer than 12 characters so this divergence never fires in `parity-grid`; not
  fixed on the Go side (decision 2 — Go is left alone).
- **Approximate caret placement inside a rendered table row.** A table line's row 1
  claims the WHOLE source line as one substituted span (decision 6); the buffer offset a
  caret would need to land on mid-cell has no exact rendered-column counterpart. Caret
  placement on a table row is therefore approximate in both implementations, and
  unreachable in the focused pane in practice (reveal is whole-block — decision 5 — so a
  rendered table row is never the row the cursor is actually drawn on).
- **`crates/rune-md/tests/table_render.rs`** is 495 lines (§1.6 limit 500) — 458 after WP6's
  own regression test for the Wrapped-layout border-synthesis bug (see the entry below),
  grown a further 37 lines since by the regression test for declining a table whose range
  starts at an unexplained mid-line position (commit `4dbbe9d`) — under
  budget, recorded here only because this plan's own WP2/WP4 work already pushed several
  sibling files over the limit (see the "§1.6 file-size overages" entry above and the
  per-work-package entries elsewhere in this file); still no new file crossed 500 lines.

## rust port — RESOLVED (2026-07-28, markdown-table plan WP6): Wrapped-layout tables synthesised a bottom border after every visual sub-row, at the wrong width

**Status:** fixed, with a regression test; found while proving the `parity-grid` gate for `tables-narrow.md` (the first real exercise of a Wrapped table through the full render -> wrap -> display pipeline — WP4's own tests covered segment `start_col`s and cell wrapping, but not `DisplaySnapshot`'s border synthesis for a Wrapped line specifically).

- **Symptom:** a Wrapped table's body row, once its cells wrap into more than one visual sub-row, rendered a `└┴┘` bottom border after EVERY sub-row instead of only the last one — and that border (and the table's top border) was drawn at the Grid layout's natural, unshrunk column widths instead of the actually-rendered, proportionally-shrunk Wrapped widths, so the border ran wider than the content it bordered and overflowed the pane. Confirmed via a real `tmux capture-pane` capture (`scripts/parity/fixtures/tables-narrow.md`), not just a unit test.
- **Root cause, part 1:** `crates/rune-syntax/src/wrap/table.rs`'s `wrap_table_line` stamped the SAME `TableSegInfo` — including `boundary` — onto every `WrapSegment` a table source line produced (row 1 plus every `extra_rows` entry). `boundary` describes the LOGICAL row's own top/bottom border membership, a property of the source line as a whole; Grid layout never produces more than one segment per line, so this was invisible until Wrapped/Pivoted (WP4) started producing several. Fixed: only the FIRST segment of a line may carry `First`/`Only` (the top border goes before it), and only the LAST segment may carry `Last`/`Only` (the bottom border goes after it) — every segment in between is forced to `Middle`.
- **Root cause, part 2:** `crates/rune-md/src/emit/table.rs`'s `emit_table` always stored the Grid layout's natural `widths` into `TableRowInfo::col_widths`, the value `DisplaySnapshot::expand_tables` uses to build a synthesised border's own text — even when the layout actually chosen for that table was Wrapped, which renders at `constrained_widths` instead. Fixed: `col_widths` now stores `constrained_widths` when the layout is Wrapped.
- **Regression test:** `crates/rune-md/tests/table_render.rs`'s `wrapped_table_gets_exactly_one_top_and_one_bottom_border_at_the_constrained_width`.

## ~~rust port — PRE-EXISTING: `reapply`'s STRICT_INVARIANTS check fires on multicursor + CRLF (recorded 2026-07-28, markdown-table plan)~~

**RESOLVED (2026-07-29, `tbl-cursor-fix`).** The CRLF/BOM framing below was
this entry's best guess at the trigger, not the actual root cause — it was
wrong. Instrumented `reapply`/`apply_edit_batch_with_cursors` directly and
reproduced with `make test-fuzz` (a fresh, simpler catch,
`no-panic-c33c6055`, now checked in as `crates/rune-fuzz/repros/
no-panic-01.rune`): the real trigger is Backspace (also Delete/DeleteWord/
outdent/delete-line), not CRLF or a BOM. `CursorSet::merge` coalesces
cursors correctly on their own PRE-edit *selection* ranges — for a
caretless cursor that's the zero-width point `[position, position)` — but
Backspace's per-cursor `bare` edit range reaches ONE RUNE LEFT of that
point (Delete reaches right; DeleteWord/outdent/delete-line reach further
still). Two cursors one rune apart never touch by `merge`'s own rule (their
points differ), so both legitimately survive as separate cursors — but the
two Backspace EDITS those cursors' commands generate DO touch
(`[start-1,start)` and `[start,start+1)`). `Buffer::apply_edits` accepts
touching, non-overlapping edits; since the earlier one is a pure deletion,
its negative shift lands the later edit's POST-edit `start` on the exact
same offset as the earlier one's — the state `reapply`'s invariant exists
to catch. So the mismatch is real (pre-edit selection space vs. post-edit
`start` space, as this entry originally said) but the boundary that
diverges is Backspace-family "reach past the cursor's own point," not
CRLF/BOM pairing (`CursorSet::merge` itself was never touched).
Fix: `crates/rune-tui/src/commands/edit_core.rs`'s new
`coalesce_touching_edits`, called from the one shared chokepoint
(`apply_edit_batch_with_cursors`) every editing command funnels through —
it unions any two edits in a batch whose `[start,end)` ranges touch or
overlap into one BEFORE the batch ever reaches `Buffer::apply_edits`,
survivor id = the lower of the two (mirroring `CursorSet::merge`'s own
tie-break). This makes "two edits share a post-edit start" unrepresentable
for every command through this chokepoint, not just Backspace. Proof:
`crates/rune-tui/src/commands/edit_core.rs`'s
`two_adjacent_cursors_backspacing_coalesce_into_one_edit_and_survive_redo`
(reverting the fix makes it fail, confirmed).

Original entry preserved below for the record.

Surfaced by the markdown-table plan's fuzz work — **not caused by it**. Verified by
replaying the script below at `a1fe09d^` (immediately before this plan's only
`rune-core` change): it fails there identically, and the fixture contains no table.

```
content # Title\n\n- item one\n- item two\n\n> a quote\n\n```rust\nfn main() {}\n```\n\n[a link](https://example.com)\n
paste line1\r\nline2
key up -a-u
key char:  ----
key char:\u{0} ----
type # 
key char:x ---u
type 
```

Violation: `NO-PANIC`, from
`reapply: two edits share a post-edit start; CursorSet::merge should have
coalesced them upstream` (`crates/rune-core/src/undo.rs`). An `alt+cmd+Up`
add-cursor-above over content holding a lone `\r` produces two cursors whose
edits land on the same post-edit start, which `CursorSet::merge` did not
coalesce.

The check itself is gated on `rune-core`'s `STRICT_INVARIANTS` (§1.3: an
ordinary build must degrade gracefully, never panic, on a producer bug —
see `crates/rune-core/src/lib.rs`'s module docs), not on `cfg(debug_assertions)`
anymore. Since a dependency does not inherit `cfg(test)`, `crates/rune-fuzz`
now depends on `rune-core` with `features = ["strict-invariants"]` explicitly
(its `Cargo.toml`) so the session fuzzer keeps exercising this check — without
that feature the violation above compiles out of the fuzzer silently and
`reapply` would instead replay the duplicate-start batch in whatever order the
tied sort produced, a possible buffer-corruption path during redo.

**Not committed to `repros/` while red** (standing convention: a repro belongs
there only in the same commit as its fix). The script above is the verbatim
copy. ~~**`make test-fuzz` stays red until this is fixed**~~ — see the
RESOLVED note above: `make test-fuzz` is green again as of `tbl-cursor-fix`.
~~Fix: make `CursorSet::merge` coalesce on post-edit start, not only on
pre-edit selection range.~~ (That was this entry's guess at the fix, before
root-causing it — see above for what the fix actually was.) Out of scope for
the table plan.

## rust port — `![[embed]]` produces no catalogue entry (recorded 2026-07-28, navigation plan WP3)

**Status:** open; disclosed gap, pinned by a test.

- **Symptom:** an embed is invisible to the navigation catalogue. `catalogue()`
  returns zero refs for `![[note]]`.
- **Root cause:** comrak's wikilink inline parser only fires when `[` is
  immediately followed by `[` AND the parser is not already `within_brackets`.
  A leading `!` opens an image bracket first, so the guard suppresses the
  wikilink match entirely and the whole construct degrades to one plain
  `Inline::Text` run — comrak never produces a `WikiLink` node to inspect.
  The `UseRole::Embed` classification (a `!`-byte test on the byte preceding
  the wikilink's own range) is implemented and correct, but unreachable for
  this spelling.
- **Consequence:** the future vault graph will be missing every embed edge.
  Obsidian counts embeds as links in its graph, so a backlinks/graph feature
  built on this catalogue would under-report until this is fixed.
- **Pinned by:** `catalogue::tests::embed_prefixed_wikilink_comrak_behaviour_is_pinned`
  asserts the current comrak behaviour, so a future comrak upgrade that starts
  emitting the node will fail the test rather than change graph output silently.
- **Fix design:** hand-parse `![[` in the same pass, the way the Go reference's
  own wikilink extension does (it carries an explicit `Embed` flag), or
  pre-scan for the construct before comrak sees it.

## rust port — navigation follow-up backlog (recorded 2026-07-28, navigation plan)

Deliberately out of scope for the navigation plan; the types exist so none of
these needs a redesign:

- **Back/forward navigation history** (back/forward). Following a link opens or reactivates a
  tab with no way back. `Destination` is the value a history stack would push.
- **Vault-wide index, backlinks and graph.** `catalogue()` is pure and takes
  `(content, blocks)`, so a headless indexer can run `parse() -> catalogue()`
  over unopened files without a viewport; nothing else is built.
- **Tree-sitter producers.** `DefRole::Symbol` and `UseRole::Import` exist and
  no producer emits them; they are what go-to-definition and import navigation
  will fill.
- **`^block` references.** `Anchor::Block` exists in the type; no producer
  emits it. The Go reference has no `^` handling either.
- **Reference-style links** `[text][ref]` and link reference definitions are
  not modelled.

## ~~rust port — CELL-ORDER violation pasting a ZWJ emoji into a list item (recorded 2026-07-28, navigation plan verification)~~

~~**Status:** open; PRE-EXISTING, proven not caused by the navigation work.~~

~~- **Symptom:** `make test-fuzz` at `RC=3000` catches
  `CELL-ORDER: row cell buf_offsets go backwards: 29 then 9`. Not caught at the
  default `RC=512`, so this is a low-frequency case the default gate misses.~~
~~- **Proven pre-existing:** the shrunk script replays identically on `f3e837d6`,
  the base commit this plan branched from, via the `replay` harness — the
  navigation work is not implicated. It is recorded here rather than ignored
  because "not caused by my changes" is never a reason to skip a failure.~~
~~- **Repro** (verbatim shrunk script; paste into `crates/rune-fuzz/repros/` to
  replay with `cargo test -p rune-fuzz --test replay`):~~

```
content 
key char:v ---u
clip \u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}\u{200d}\u{1f466} family
key tab ----
key delete ----
key backspace ----
type - 
key right ----
key right ----
```

~~- **Resulting buffer:** `"- \u{200d}👩\u{200d}👧\u{200d}👦 family"` — note the
  leading `👨` was already consumed by the backspace, leaving the content
  starting on a bare ZWJ joiner. The cell builder then emits a row whose cell
  `buf_offset`s are not monotonic.~~
~~- **Likely area:** grapheme-cluster segmentation in the cell/render path when a
  cluster begins with a lone ZWJ. Adjacent to the previously-fixed ZWJ
  grapheme-corruption entry in this file, so treat that fix as incomplete
  rather than unrelated.~~
~~- **Not committed to `repros/` while red**, per the standing convention that a
  repro lands in the same commit as its fix. The proptest regression seed was
  likewise not committed, so the default gate stays green rather than
  deterministically red for unrelated work.~~
~~- **Out of scope** for the navigation plan.~~

**RESOLVED (2026-07-29, defect-fix session).** A second, independently-found
repro (`content Hello there...`, pasting the same ZWJ family emoji into
prose, then `[a](b)` + CJK text) hit the identical invariant
(`buf_offsets go backwards: 110 then 105`), confirming one shared root
cause rather than two bugs.

**Root cause was NOT "a cluster begins with a lone ZWJ" mishandling** — a
lone ZWJ's own cell (width clamped to 1 so it still gets a reachable caret
column) was already correct. The real defect: `rune-tui::render::
segment_cells_with` grapheme-segments each `SyntaxSpan`'s own text
INDEPENDENTLY (so a cluster can never straddle two spans — a `Substituted`
span's `cell_map` only indexes its own chars), but `rune-syntax::wrap`'s
width/coordinate math — `wrap_line`'s greedy breaker and `query.rs`'s
`visual_col`/`byte_col_from_visual` — concatenated every span's text into
ONE string before grapheme-segmenting it, letting a cluster join ACROSS a
span boundary. A ZWJ makes this land reliably: Unicode's own grapheme rule
(UAX #29 GB9) joins a ZWJ to WHATEVER character precedes it, unconditionally
— span boundary or not — so whenever a span boundary happened to sit right
before a lone ZWJ (a concealed link's visible text or a list marker span
followed immediately by pasted ZWJ-emoji content), the wrap-computed visual
column undercounted by exactly that fused cluster's width and diverged from
the row's real cells. `render::place_caret` then couldn't find any cell at
the (wrong) computed column and fell back to appending a synthetic caret
cell at the row's END — with the cursor's real, smaller `buf_offset`,
landing after already-emitted higher `buf_offset` cells: the `CELL-ORDER`
violation.

**Fix:** `crates/rune-syntax/src/wrap/mod.rs` adds `next_grapheme`, the one
shared chokepoint every cross-span text walk in this crate now calls: it
clamps a cluster read to never cross a span's own end boundary, matching
`render.rs`'s per-span segmentation exactly. `wrap_line`'s greedy breaker and
`query.rs`'s `visual_col`/`byte_col_from_visual` (`spans_text_and_bounds`,
replacing the old `spans_text`) were rewritten to use it instead of a bare
`graphemes(true)` walk over the concatenated text.

- Regression test: `crates/rune-syntax/src/wrap/mod.rs`'s
  `visual_col_does_not_fuse_a_zwj_across_a_span_boundary` — builds a
  `Substituted` span immediately followed by an `Identical` span starting
  on a lone ZWJ and asserts `visual_col` computes the per-span total (4),
  not the fused-cluster undercount (3). Verified failing (asserting 3) with
  the fix reverted, passing with it restored.
- Repros landed: `crates/rune-fuzz/repros/cell-order-02.rune` (prose +
  link + CJK) and `cell-order-03.rune` (list item), alongside the fix.

## rust port — §1.6 budget: `rune-syntax/src/wrap/mod.rs` grew past the ceiling fixing CELL-ORDER (recorded 2026-07-29)

- `crates/rune-syntax/src/wrap/mod.rs` is 587 lines (§1.6 limit 500; was 489
  before this fix) — grown ~98 lines by the CELL-ORDER fix above: the
  `next_grapheme` chokepoint (with its doc comment explaining why every
  cross-span text walk needs it) plus the new
  `visual_col_does_not_fuse_a_zwj_across_a_span_boundary` regression test
  and its doc comment. The unit tests are the larger share of the growth
  and already follow this crate's own established pattern (`WrapMap`'s
  other hand-built-`SyntaxLine` tests live in this same `mod tests` block);
  moving them to a `tests/` integration file would need `WrapSegment`,
  `slice_spans`, and the `query::visual_col`/`byte_col_from_visual`
  `pub(super)` free functions exposed more widely than this crate wants
  today. Split when next touched, mirroring this file's own prior
  `wrap/table.rs` extraction: `next_grapheme` plus `grapheme_width`/
  `grapheme_width_with_tab`/`control_aware_width`/`rune_width_with_tab`
  (the whole shared width/cluster-boundary chokepoint, used by both this
  file and `query.rs`) could move to a sibling `wrap/width.rs`.

## rust port — §1.6 budget: `rune-cli/src/main.rs` and `explorer.rs` grew in the navigation plan (recorded 2026-07-29)

- `crates/rune-cli/src/main.rs` is 521 lines (§1.6 limit 500; was 429 before this
  work) — grown by WP7's strict-CLI wiring: `LaunchAction` dispatch, `-w`
  validation, workspace-root resolution and the multi-file open loop. The parser
  itself already lives in its own `cli.rs`; the remainder is startup sequencing
  that has to happen in `main`. Split when next touched: the recovery-store
  bootstrap (`bootstrap_db`/`DbBootstrap`, roughly 150 lines) is a self-contained
  unit that could move to a sibling module.
- `crates/rune-tui/src/explorer.rs` is 588 lines (§1.6 limit 500; was 553
  immediately before this work) — grown 35 lines by WP4.S5's `app.root` fallback
  in `initial_root` plus its three new tests. Same split suggestion as the older
  entries above: `EXPLORER_BINDINGS` + `handle_key` are a self-contained unit.

## rust port — `App::db_load_versions` shadows `db_ops` (recorded 2026-07-29, navigation plan code review)

**Status:** open; newly-introduced tech debt, no defect observed.

`crates/rune-tui/src/db.rs`'s `load_document` inserts into BOTH `App::db_ops`
(`op id -> DocumentId`) and `App::db_load_versions` (`op id -> issue-time buffer
version`), and `handle_db_event` must remove from both on every terminal arm.
Nothing structurally enforces that the two maps stay in step — only convention
at four call sites — which is exactly the parallel-source-of-truth shape the rest
of this codebase avoids. Fix: give `db_ops` a richer value type (e.g.
`PendingOp { doc: DocumentId, issued_version: Option<u64> }`) so one map carries
both facts and the two cannot drift. Deferred because `db_ops` is read by
`save.rs` and `rename.rs` too, so the change is wider than the navigation plan's
scope.

## rust port — `Mem::stat` re-derives `read_dir`'s directory test (recorded 2026-07-29, navigation plan code review)

`crates/rune-vfs/src/mem.rs`'s `stat` decides "is this a synthetic directory" with
its own `strip_prefix`-based scan over `state.files`, while `read_dir` a few lines
below computes the same fact in a different shape. Two implementations of "does a
stored key sit strictly below this path" that can disagree if only one is edited.
Fix: extract one private helper both call.

## rust port — `Anchor::Line` degrades to an empty-string match (recorded 2026-07-29, navigation plan code review)

`crates/rune-tui/src/navigate.rs`'s `anchor_name` maps `Anchor::Line(_)` to `""`,
and heading lookup compares normalized names — so a line anchor would match a
heading whose name normalizes to empty rather than resolving by line number.
Latent only: no producer emits `Anchor::Line` today. Fix it when the first one
does, by giving line anchors their own lookup path instead of a name comparison.

## ~~rust port — TABLE-ROW-WIDTH: a table row with ZWJ emoji / CJK cells is wider than its own border (recorded 2026-07-29, markdown-table plan)~~

**RESOLVED (WP9.S1, code-review defect-fix session).** Root cause: Grid's
`col_widths` measured a column's width by grapheme-segmenting a cell's
JOINED text in one pass, but `grid_row` renders a cell by grouping its
per-char scope into `group_runs`' maximal same-scope runs FIRST, then
grapheme-segmenting EACH run independently. A grapheme cluster straddling a
scope change inside a cell (a ZWJ-joined emoji pair split across an
emphasis-run boundary — the same class `e3238fa` fixed in
`rune_syntax::wrap`) disagreed between the two: the joined pass can fuse the
cluster across the run boundary (UAX #29 GB9/GB11 join a ZWJ to whatever
precedes it, unconditionally, span/run boundary or not); the per-run render
never can. Fix: `cell_display_width` builds the SAME `group_runs` grouping
the renderer uses, then sums each run's own grapheme width — measurement and
rendering now share one code path. Regression test:
`crates/rune-md/tests/table_render.rs`'s
`zwj_family_split_by_emphasis_and_cjk_row_widths_agree`, confirmed red
against the pre-fix `col_widths` (asserted separator/header width mismatch,
16 vs 13) before the fix landed.

~~**Status:** open. A defect in the markdown-table rendering work itself, not a
pre-existing one — `TABLE-ROW-WIDTH` is that work's own invariant over its own
table layout. Surfaced by `make test-fuzz RC=5000` only after three earlier
fuzz bugs stopped short-circuiting the run.~~

~~- **Symptom:** `TABLE-ROW-WIDTH: table_group 0: row 1 has summed width 123, but
  row 0 (same group) has width 79`. Row 0 is the synthesised top border, row 1
  a content row — the box does not line up, by 44 cells.~~
~~- **Verified not caused by the ZWJ span-boundary fix:** the shrunk script
  replays identically at `bf3e7e0`, immediately before that fix.~~
~~- **Likely area:** the table layout measures column widths from its own
  rendered cell text, while the border row is built from the stored
  `col_widths`. A cell holding a ZWJ emoji or CJK is the divergence point — the
  same class as the already-fixed tab-in-a-cell mismatch, where measurement and
  rendering disagreed about one glyph's width. Note the span-boundary fix
  corrected the WRAP layer's walkers; table `col_widths` and the renderer's own
  per-span segmentation were not touched and may still disagree.~~
~~- **Repro** (verbatim shrunk script; paste into `crates/rune-fuzz/repros/` to
  replay):~~

```
content \u{feff}hello
type \u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}\u{200d}\u{1f466} family aA1! \u{4f60}\u{597d} \u{1f642} mix
paste 
key char:z ---u
key char:z ---u
key char:z s--u
key char:z s--u
type \n\n\n
key left ----
key left ----
key left ----
key home ----
key down ----
paste 
key up ----
type \u{4f60}\u{597d}\u{4e16}\u{754c}\u{ff0c}\u{4e16}\u{754c}\u{4f60}\u{597d}
type hello world \u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}\u{200d}\u{1f466} family
paste line1\r\nline2
key pageup s---
key end s---
paste hello world
type \u{4f60}\u{597d}\u{4e16}\u{754c}\u{ff0c}\u{4e16}\u{754c}\u{4f60}\u{597d}
type 
type \u{e9} \u{e0} \u{f4} hello world
key left ----
key left ----
key left ----
key left ----
key left ----
key left ----
key left ----
type \u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}\u{200d}\u{1f466} family
key up ----
key down ----
key down ----
type | :-: |
key left ----
key left ----
key down ----
paste hello world
```

~~- **Not committed to `repros/` while red**, per the standing convention that a
  repro lands in the same commit as its fix. **`make test-fuzz` stays red until
  this is fixed**; every other gate (`fmt`/`lint`/`build`/`test`/`perf-guard`/
  `replay`/`parity-grid`) is green.~~
## rust port — `db_wiring` flakes under parallel test load (recorded 2026-07-28, tree-sitter plan, WP5.S5/S6 split)

Observed three times now, twice while other work was compiling concurrently in
another worktree and once during an otherwise ordinary sequential run; 12/12
clean on immediate re-runs. The second
occurrence was a bare `test result: FAILED` count with the name not captured,
so "it is `db_wiring`" rests on the first observation alone — treat the
attribution as unconfirmed and re-derive it from a captured failure before
fixing. The load correlation, however, reproduced. The file is not touched by
the tree-sitter work.

The test already follows the repo convention — it waits on bounded spins, not
wall-clock sleeps, and says so. So the defect is not a sleep to delete: it is a
spin bound calibrated on an unloaded machine, which a loaded one can exhaust
before the writer thread posts its `DbEvent`.

**Not fixed here, and deliberately not guessed at.** Widening the bound without
a reproduction would be a spot-fix against a symptom, and the failure has not
been reproduced under load with polled evidence of *which* spin ran out. The
right fix is to make the wait condition-driven rather than iteration-bounded, so
load cannot change the outcome — but that needs the repro first.

## rust port — a lone `\r` projects a comrak line onto the wrong buffer line (recorded 2026-07-28, found by the session fuzzer during the tree-sitter plan)

`make test-fuzz` is RED on this. Not caused by the tree-sitter work — WP7's new
seeds and actions merely gave the fuzzer the reach to find it.

Two line indexes exist: `line_starts` splits on `\n` only (the buffer/editor
model, §1.5) while `comrak_line_starts` follows CommonMark, where a bare CR
also ends a line. CRLF is safe — both agree. A LONE `\r` is where they diverge.
`per_line_content` partitions correctly in comrak space, then `emit_table`
reinterprets that partition element as a buffer line. A paragraph and a table
header row then share one buffer line and render as one display row, so the
row is wider than the border synthesised from its own `col_widths`.

Reachable by an ordinary user, no fuzzing required: paste CRLF text, backspace
over the line break, and the `\r` survives alone (paste is verbatim by §1.4.5,
correctly). Opening a CR-only file does the same. The render layer maps `\r` to
zero cells, so there is no glyph explaining the shift. No panic and no data
loss — the bytes are intact. It is a rendering-integrity defect.

`emit/walk.rs`'s task-checkbox path has the identical `line_at(starts, …)`
shape, so this is a class, not one call site.

**Fix, when scheduled:** parse a LENGTH-PRESERVING shadow copy (`Cow<str>`,
allocated only when a lone CR is actually present) in which every lone `\r`
becomes a space. One byte in, one byte out, so every sourcepos-derived offset
stays valid against the real buffer and the user's bytes are never touched —
a parse-time view, not a mutation, so §1.4.5 is untouched. The dual index then
ceases to exist and the mis-projection becomes unrepresentable. It changes what
`x\r| a | b |` parses to, so **check `golang/` for parity before committing** —
that is why it is not being done inside the tree-sitter plan.

Deliberately NOT fixed by measuring the border from the rendered row: that
would satisfy the invariant by construction while painting a neat box around
foreign text, and blind the only detector of the real defect.

## rust port — wrapped table continuation rows each draw their own top/bottom border (recorded 2026-07-28, same investigation)

Pre-existing and independent of the above. `wrap_table_line` clones the whole
`TableSegInfo` — `boundary` included — onto every continuation segment, and
`expand_tables` keys `starts_table`/`ends_table` purely off `boundary`. Its
`Middle` branch guards with a `prev_line != model_line` check; those two do
not. A 40-column wrapped table therefore emits a `└┴┘` after every continuation
row and a `┌┬┐` before every one when the header wraps.

Invisible to `TABLE-ROW-WIDTH` — every width agrees — but plain garbage on
screen.

**Fix:** make `TableSegInfo::boundary` an `Option<RowBoundary>` and have
`wrap_table_line` emit `None` for continuation segments, so a continuation row
cannot claim to be the table's first or last.

## rust port — tree-sitter highlighting: unmeasured budgets and known-open gaps (recorded 2026-07-28, tree-sitter plan WP8)

- `PARSE_BUDGET` (`crates/rune-tui/src/runtime/highlight_cmd.rs`, 5s, now
  applied once per code region) and `MAX_SPANS`
  (`crates/rune-ts/src/highlight.rs`, 100_000) are constants chosen without
  profiling a real large document — not measured against any target frame
  budget or memory ceiling. Revisit once a slow-parse or span-flood case is
  actually observed, rather than tuning blind.
- The Terraform highlights query (`crates/rune-ts/queries/terraform.scm`) is
  hand-authored in-repo, offline, from the grammar's own node kinds —
  `tree-sitter-hcl` ships no highlights query at all. It is coarser than an
  upstream community query would be and can drift from the grammar with no
  upstream signal to catch it.
- `crates/rune-fuzz` now transitively links 21 grammar crates through
  `rune-tui`. A grammar's C `ts_assert` firing `SIGABRT` during
  `make test-fuzz` kills the process before `tests/human_session.rs` writes
  its artifact bundle — no shrunk input, no `script.rune`, no `repros/`
  promotion path for that failure. The fuzzer never calls `rune_ts::highlight`
  itself (it only delivers synthetic `Action::Highlight` replies), so the
  exposure is confined to real `Cmd` worker threads, not the fuzzer's own
  driver loop.
- `scripts/parity/fixtures/fences-code.md` is excluded from
  `make parity-grid` (`scripts/parity/grid.sh`'s `excluded_reason`) because Go
  has no tree-sitter — its fenced-code token colours cannot be asserted
  against a Go capture at all, not even as a byte-diff.
- `make parity-grid` compares only `emphasis.md` today — nine of its ten
  listed fixtures are excluded (see `scripts/parity/README.md`'s "Known
  divergences"). It is retained as a cheap regression tripwire on markdown
  rendering, not as evidence that Rust and Go screens actually match; treat a
  green `make parity-grid` accordingly, and see plan Risks for the same
  caveat.

## rust port — findings from the lone-`\r` fix that were out of its scope (recorded 2026-07-28)

Recorded together because one fix surfaced all of them.

**`crates/rune-md/TODO.md`'s residual strict-invariants repros: two now pass.**
`"- >_\r\tx\n]"` and `">\t<b>\ra"` both involved a lone `\r` and are closed by the
shadow-copy parse. `"- >👍\n\tx\nc"` still panics — it is tab-stop expansion with
no CR anywhere, genuinely separate. Someone who owns that file should strike the
first two and keep the third.

**A GFM table row with MORE cells than its header silently drops the extra
cell's text at emit time**, with no fallback. Reproduces with plain pipes, no CR
involved. Found while probing fixtures; not investigated further.

**`crates/rune-tui/tests/tui_render.rs` is now 555 lines**, over §1.6. It was
already 520 before this branch; the lone-CR render test pushed it further. Split
it when next touched.

**`make parity-capture && make parity-assert` fails on a "rust bottom content
row (line 33)" width mismatch.** Confirmed pre-existing — reproduces with the
lone-`\r` fix reverted — and on one run the *Go* side failed too, which points
at capture-harness flakiness (terminal-size timing under the sandbox) rather
than a Rust regression. `make parity-grid` passes. Needs its own investigation
with polled evidence before anyone changes rendering code to chase it.

## ~~rust port — a second `TABLE-ROW-WIDTH` instance, zero-width-space flavoured (recorded 2026-07-28, found by the session fuzzer)~~

**RESOLVED (WP9.S1, code-review defect-fix session).** Same `TABLE-ROW-WIDTH`
invariant, same architectural root cause as the ZWJ/CJK entry above (Grid's
`col_widths` measuring a cell's JOINED text instead of its rendered
per-scope runs) — closed by the same `cell_display_width` fix. Verified
directly: decoding this entry's own shrunk script and driving it through
`rune_fuzz::driver::run` no longer raises `TABLE-ROW-WIDTH` (or any other
invariant) against the fixed tree.

~~The lone-`\r` shadow-copy fix closed the CR-driven instance of this invariant.
A fresh-seed `make test-fuzz` then found another: `table_group 0: row 3 has
summed width 18, but row 2 (same group) has width 17`. Different mechanism —
no CR anywhere; the script types two U+200B ZERO WIDTH SPACEs into a table
cell. A zero-width character contributes 0 terminal cells but is 3 bytes, so
this is a live §1.5 bytes-vs-cells hypothesis worth checking first.~~

~~`RS=1 make test-fuzz` passes; this needs fresh seeds to surface.~~

Shrunk script (12 steps, decodable by `rune_fuzz::script::decode`):

```
content # Doc\n\n| Name | Age |\n| :--- | ---: |\n| Alice | 30 |\n| Bob | 25 |\n\ntail\n
type 
key backspace ----
key down ----
key right -a--
key left s---
type  \u{200b}\u{200b}  
key char:z ---u
type 
type 
paste 
key up ----
```

~~**Deliberately NOT checked into `repros/`** — that directory's contract is that
every script in it replays clean, and this one does not until the bug is fixed.
It belongs there in the same commit as its fix.~~ (Checking the script itself
into `repros/` is `rune-fuzz`'s call, outside this fix's crate — left to
whichever WP next touches that crate.)

## rust port — open findings from the second code review (recorded 2026-07-29)

Three items from that review were fixed in the same commit that records this
entry (the missing `return` bounding the highlight retry chain, a stray
`path:line` citation, and this note). The rest are open:

**`make test-fuzz` mutates a tracked file, so the gate list is not idempotent.**
A random-seed run appends its failing seed to
`crates/rune-fuzz/proptest-regressions/human_session.txt`. Running the
documented gates in order therefore leaves a dirty tree AND makes the
subsequent `RS=1 make test-fuzz` replay the brand-new seed and fail — which
looks like a pinned-run regression and is not one. Either the gate list should
run the pinned form first, or `test-fuzz` should write its persistence file
somewhere untracked.

**A token spanning a line boundary inside a blockquoted fence still swallows
the `"> "` prefix.** The per-line fence fix closed the common case, but
`map_reconstructed_span` maps start and end independently, so a token crossing
the gap between two non-contiguous lines yields a buffer range covering the
prefix between them. Measured: a `/* one\n> two */` comment gives span
`12..27` = `"/* one\n> two */"`, and the overlay then paints the `>` marker.
Cosmetic, and strictly better than the bug it replaced.

**`crates/rune-tui/tests/highlight.rs` is 776 lines** (477 before the fix
round), well over §1.6. It is the largest file in that change set. Split it
when next touched — the fence, retry, clamp and overlay cases are four
natural files.

**`MAX_SPANS` truncation is still invisible to the user.** `HighlightState`
carries the flag and it is testable, but nothing renders it, while the sibling
timeout does surface a status line. Half-closed.

**The two highlight reply handlers are near-duplicates.** `handle_highlighted`
and `handle_highlight_retried` differ only in their terminal action and in
whether the `return` before the shared `pending` tail is present — the exact
divergence that produced the bug fixed above. One variant carrying an
`is_retry` flag would make that class unreachable rather than repeatedly
caught.

**Does coalescing touching per-cursor edits intend to drop a cursor?** Two
touching cursors collapse to one survivor (lower id), mirroring
`CursorSet::merge`, but no test asserts the post-edit cursor count. Confirm the
shrinking count is the intended editing UX rather than an unexamined side
effect.

**`rune-syntax`'s `ScopeTable`/`scope_table()` vocabulary is open by API,
closed in practice — decide before a third producer lands.** Every current
producer (`rune-md`'s comrak emitter, `rune-ts`'s tree-sitter one) builds its
table from the single `scope_table()` constructor over the fixed
`MARKDOWN_SCOPES`/`CODE_SCOPES` lists, never calling `ScopeTable::register`
with a name of its own; `rune-ts`'s `highlight.rs` silently drops (via
`continue`, no span emitted) any capture whose name doesn't resolve against
that shared table — a grammar with legitimate captures outside both lists
loses them with no signal to the user, not even a truncation-style flag.
This was flagged as a same-producer-vector-index-collision risk before the
rune-ts merge landed; that specific risk is now moot (both producers agree
on ids by construction, pinned by `markdown_scopes_still_start_at_id_zero`).
The open question that remains: should an unresolvable capture register
itself into a per-call table instead of being dropped (truly open
vocabulary, but then a theme's `scopes: Vec<Style>` built from the shared
`scope_table()` no longer covers every id in play), or should the closed
list simply grow to include whatever `rune-ts`'s actual grammars need
(simpler, but ties this crate's vocabulary to tree-sitter capture-naming
conventions)? Needs a decision, not a silent default, before wiring up a
grammar whose captures actually exceed today's list.
## rune-db — §1.6 file-size overages (recorded 2026-07-29, code-review WP6)

Six files in `crates/rune-db/src` are over the §1.6 500-line ceiling. None of
this is newly introduced by WP6 — all six were already over budget per
`CODE-REVIEW.md`'s rune-db finding 10 — but WP6's own fixes (the `commit_save`/
`record_fresh` transaction-merge, `capture_and_rebind`, the `paths.rs`
chokepoint call sites, the `evict_path_claim_tx`/`set_identity_tx` extraction,
and the new regression tests) grew four of the six further. Recording the
current sizes and a split direction for each, per the house rule ("never
silently skip it").

- `crates/rune-db/src/writer.rs` is 988 lines — the writer thread's own op
  dispatch (`OpKind` match), idle maintenance, shutdown/TRUNCATE sequencing,
  and panic-guard machinery, plus its own substantial unit test module. Split
  direction: the panic-guard/`fatal`/shutdown-sequencing trio is a
  self-contained state machine that could move to a sibling
  `writer_lifecycle.rs`, leaving `writer.rs` itself as just the `OpKind`
  dispatch table.
- `crates/rune-db/src/materialize.rs` is 954 lines (grown from 813 by WP6.S1's
  transaction-merge, WP6.S4's path/row-agreement check, WP6.S6's
  `evict_path_claim_tx`/`set_identity_tx` extraction, and three new
  regression tests). The real coupling CODE-REVIEW.md already named: CAS
  refusal, swap-race, `materialize_create`, `commit_save`, and the
  rebind/evict chokepoints are separable units sharing only `DocSession`/
  `WriteIntent`. Split direction: move `materialize_create` +
  `rebind_document_tx`/`evict_path_claim_tx`/`set_identity_tx` to a sibling
  `rebind.rs`, leaving `materialize.rs` as the CAS overwrite path
  (`materialize`/`materialize_overwrite`/`record_fresh`/`commit_save`) plus
  its own tests.
- `crates/rune-db/src/rename.rs` is 685 lines (grown from 628 by WP6.S3's
  `capture_and_rebind` transaction-merge and its expanded doc comment). Split
  direction: `rename_bind`/`rename_replace` are the two public entry points;
  the shared `rebind`/`capture_and_rebind` primitives plus their tests could
  move to a sibling `rename_bind.rs`/`rename_replace.rs` pair, mirroring
  `adopt.rs`'s tx-primitive/standalone-wrapper split.
- `crates/rune-db/src/store.rs` is 657 lines (grown from 621 by WP6.S5's
  fallible `reader_target` conversion in `open_ladder` and its new
  corrupt-DB-file regression test). Split direction: `open_ladder`/
  `open_file_backed`/`open_memory_backed`/`memory_uri` (the open-ladder
  primitives) are already a fairly self-contained unit that could move to a
  sibling `open_ladder.rs`, leaving `store.rs` as the `Store` type and its
  op-enqueue methods.
- `crates/rune-db/src/load.rs` is 668 lines (grown from 653 by WP6.S2's
  reap-vs-hydration re-verification in `find_inheritable_draft`). Split
  direction: `find_inheritable_draft`/`most_recent_session_for_doc`/
  `is_session_alive` (the cross-session inheritance decision) are a
  self-contained unit that could move to a sibling `inherit.rs`, leaving
  `load.rs` as just `load` itself.
- `crates/rune-db/src/journal.rs` is 611 lines — untouched by WP6. Split
  direction unchanged from the review: `append_edit`'s coalescing/truncation
  logic vs. the plain `undo_peek`/`redo_peek`/`move_undo_pos`/`current_seq`
  readers are separable; the former could move to a sibling
  `journal_append.rs`.

None of these six are split in this work package — WP6's scope is the
findings themselves, not the pre-existing (and self-inflicted) file-size
churn; recording per house rule rather than silently skipping it.
**`crates/rune-tui/src/keymap/editor_bindings.rs` is 508 lines** (452 before),
over §1.6. Adding the `alias` field to `Binding` cost one line per literal and
this table holds 56 of them — the file crossed the budget on a purely
mechanical change, with no new behaviour to justify a split. Split it when next
touched; the motion, selection, editing and clipboard chords are four natural
groups. `explorer.rs` grew the same way (588 -> 594) but was already tracked.

**Shrinking a pane's axis past a dragged split collapses the trailing pane,
and that is deliberate.** Drag the Files/Open divider down, then shorten the
terminal below what the drag asked for: the tab rows disappear. This is the
stated collapse rule — below its floor, a collapsible pane goes — applied to a
size the frame can no longer grant. It is transient: the dragged size is never
written down, so restoring the height restores both sections untouched.

The friendlier-looking alternative, sparing the trail whenever the request no
longer fits, was implemented and then reverted. `allot` cannot tell a drag from
a resize — both reach it only as `(desired, available)` — so sparing the trail
on every over-ask also spares it on an *overshooting drag*, and overshoot is how
the collapse gesture is actually performed: nobody lands the pointer on the one
row that leaves the trail a single cell under its floor. The cost was losing
drag-to-collapse for the tab rows entirely, while a unit test calling `allot`
directly with a hand-picked value still passed and hid it.

Doing it properly means recording the intent where it is known — at `request`
time, reached only from a drag — instead of re-deriving it in `allot` on every
read: give the splitter an explicit "trail collapsed by the user" state rather
than inferring collapse from a transient `available`. That is a design change to
`Split`'s API (`request` would need the axis length and the trail's limits), so
it is recorded here rather than rushed.

## syntax-highlighting-latency plan, WP3 — the per-frame viewport query joined the render budget

`render::build_rows` now runs a `rune_ts::highlight_range` query, scoped to the visible byte window, on every frame for a code document with a retained tree — see `crates/rune-tui/src/render/mod.rs`. This is new per-frame cost with no dedicated gate (`make perf-guard` only covers `rune-md`'s parse pipeline). When the display-pipeline budget review already tracked elsewhere in this file happens, it must measure this query alongside the existing whole-document `build_rows`/snapshot recompute — not just the pre-existing cost.

## no fuzz invariant delivers a tree-backed highlight reply yet

The session fuzzer's `HL-CLAMPED`/`HL-STALE-DROP`/`HL-NO-REFLOW` invariants (`crates/rune-fuzz/src/invariant/highlight.rs`) now read `highlight::visible_spans` — the same query the renderer runs — so they cover the render-time clamp, sort and window filter for every channel, including the tree-backed path when a session happens to produce a code region. What they still never do is DELIVER a tree: `Action::Highlight` injects hostile spans, because a `ParsedTree` cannot be synthesized. A fuzz action that runs a real `rune_ts::parse` over a small fixture and delivers the resulting `RegionPayload::Tree` would close that gap; it is future work, not part of this change.

## `crates/rune-tui/src/runtime/mod.rs` is 542 lines (§1.6 limit 500; recorded at the rr/integration merge)

Already pre-existing-overage territory before this merge (531 lines on the integration side, which had grown `Msg` with `FileOpened`/`RenameDone`/`DirLoaded` and their `Cmd` constructors past the ceiling on its own). Folding in the instant-open highlight rework's `FIRST_PAINT_BUDGET` re-export and the `first_paint_highlight` bootstrap call added another ~11 lines. Split direction: the `Msg`/`Cmd`/`Effects` type definitions and the `run` main-loop function are two fairly separable concerns; the per-`Cmd`-kind constructors already live in sibling modules (`highlight_cmd.rs`, and similarly named ones for rename/save/dir-load) — moving the bootstrap sequence (`first_paint_highlight`'s call site plus its surrounding setup) into its own `bootstrap.rs` would recover most of the overage without touching the `Msg`/`Cmd` types themselves. Not split here — out of scope for a merge-conflict resolution.
