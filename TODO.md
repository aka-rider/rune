## Open

- [ ] **`COPY_CTRL_SHIFT` (`crates/rune-tui/src/keymap/editor_bindings/clipboard.rs`, `Char('c')+CTRL_SHIFT`) is unreachable.** This crate requests `REPORT_ALTERNATE_KEYS`, under which a shifted chord arrives as the shifted character itself with `SHIFT` cleared, not as the plain character with a `SHIFT` bit set — so a `CTRL|SHIFT` row on `Char('c')` can never match what actually gets delivered. Discovered while binding the in-file search next/prev chords, which hit the identical trap and were bound correctly instead (`Char('G')+CTRL`, not `Char('g')+CTRL_SHIFT`). Out of scope to fix mid-feature; the row needs re-keying to whatever the shifted-char delivery actually reports, alongside a regression test proving it fires (the positive-resolve pattern `global.rs`'s `ctrl_shifted_g_resolves_to_search_prev` now uses).

- [ ] **`crates/rune-tui/src/runtime/mod.rs` is over the 500-line file budget (612 lines, already 572 before this change).** Search history's `Msg::SearchHistory`/`CmdKind::SearchHistory`/`load_search_history_cmd` added another `Cmd` family on top of an already-over-budget file. Not split here (that change's own scope is the search feature, not this file's pre-existing overrun) — a follow-up should split `Msg`/`CmdKind`/`Cmd`/`Effects` (the runtime's core vocabulary) from the per-feature `*_cmd` constructor functions (`load_dir_cmd`, `read_file_cmd`, `read_preview_cmd`, `load_search_history_cmd`, ...), which have no reason to share a file with the types they return.

- [ ] **`runtime::highlight_cmd` has a flaky test under `cargo test -p rune-tui`.** Observed intermittently in the full-crate run, reproduced on pre-change commit `8620c3b` (so pre-existing, not introduced by the review-fix merges), and passes reliably when run in isolation — the symptom points at timing/ordering coupling across tests rather than a bug in the highlight logic itself. Needs a deterministic rework (drop whatever cross-test or wall-clock dependency lets the outcome vary) rather than a rerun-until-green workaround.

- [ ] **`crates/rune-tui/src/layout.rs` is over the 500-line file budget (564 lines).** The Enter/Escape rework's narrow-frame flip (`LayoutMode::ExplorerOnly`) added a `carve_column` helper shared between the ordinary `Split` path and the new full-width column path, plus the extra `Resolved.mode` bookkeeping and test coverage for the flip. Not split out here to avoid destabilizing the one geometry chokepoint (`resolve`/`geometry`) mid-rework, alongside a concurrent Explorer-side rework on a sibling branch touching the same area; a follow-up should extract `carve_column` (and maybe the whole `Resolved`-building match) into a sibling module the way `split.rs` was already split out.

- [ ] **`cluster_highlight` (`crates/rune-fuzz/src/generate/cluster.rs`) no longer restores focus from the Explorer/Tabs before its guaranteed `Key('h')` edit.** Deleting `GlobalCommand::FocusEditor` (`^E`, Enter/Escape rework) removed the only unconditional, toggle-free way for a STATIC generator (no live `app.focus()` to check) to guarantee landing on the Editor from any pane; `^B`/`GlobalCommand::ToggleLeft` is a genuine toggle and cannot safely replace it blindly — pressed unconditionally it would as often steal focus from an already-focused editor as reclaim one that wasn't. `cluster_chrome`'s `Key(CTRL_B_KEY)`/`Key(CTRL_T_KEY)`/`EXPLORER_SEARCH_KEYS` arms can each leave focus on the Explorer or Tabs ahead of a later `cluster_highlight`; only the `Key(CTRL_R_KEY)` (Title-parking) case is still corrected, via `ESCAPE_KEY` alone. Not a caught invariant violation — the stray `h` keystroke still stays inside whichever pane it lands on (`PANE-NO-BLEED` holds) — just a quieter, uncaught session for the `HighlightVersion::Stale`-vs-`Live` distinction that cluster exists to exercise. A real fix needs `cluster_highlight` (or the session assembler in `generate.rs`) to know which pane the immediately preceding cluster left focus on, not a single blind keystroke.

- [ ] The title field has no horizontal scroll: an over-long file name is clipped by `Paragraph`, not scrolled to follow the cursor, so editing the tail of a name wider than the terminal is awkward. A viewport can be added to `TextField` later; not attempted here to avoid desyncing byte offsets from a truncated string.

- [ ] **A fence inside a list item leaks the item's continuation indent into the source handed to tree-sitter.** The per-line content split drops only prefixes it can attribute to a *repeating* container marker, and a list item's indent is not one, so a fence in a list reconstructs as `"  let x = 1;"` rather than `"let x = 1;"`. This contradicts the stated invariant that container prefix bytes must never reach a parser as source. Harmless for a brace-delimited grammar, which absorbs the indent through error recovery; an indentation-sensitive one (YAML, Python) loses most of its structure to it. Pre-existing — surfaced, not caused, by the `CodeRegion` work, and pinned by a test in `crates/rune-md/tests/code_regions.rs` so a fix is a visible decision rather than a silent drift. The fix belongs in the per-line splitter's notion of an attributable prefix, not at a call site.

- [ ] **A highlight region's retained tree is found by position, so adding or removing a region reparses everything below it.** `plan_jobs` looks up the candidate tree at the same index in document order; reuse is then gated on `tree.source() == source.text`, so a wrong candidate is rejected rather than misapplied — a missed-reuse cost, never a correctness bug. It matters because it is exactly the case the pass ceiling has to absorb: typing an opening fence at the top of a document invalidates every fence beneath it. A content-keyed lookup is not a drop-in — `install_regions` inherits each slot's channels *by the same index*, so matching at a different index would hand a region another region's colours. Doing it properly needs an explicit inherit-from index carried through the reply plus claim bookkeeping for two regions with identical content, since the trees cannot travel to the Cmd thread (the render path paints from them). Judged more machinery than the reuse currently buys.

- [ ] **An indented code block's first content line starts past its four-space indent while every continuation line keeps its own.** The shared per-line splitter trusts the block's own start offset for the first line (comrak reports it past the indent) and falls back to the physical line start for the rest, so the block reconstructs as `"let x = 1;\n    let y = 2;"`. Nothing observable today — an indented block carries no info string, so the highlighter drops it — but it becomes reachable the moment a consumer reads an untagged region's text. Pre-existing and shared with every other multi-line construct; pinned by a test alongside the item above.

- [ ] **The input reader thread pushes into an unbounded `mpsc` with no backpressure**, and `runtime::run` drains the whole backlog into one batch applied serially — so a key can be handled arbitrarily long after it physically arrived, with nothing bounding how deep the backlog can grow.

- [ ] **`after_update` clones the whole buffer (`buffer.content().to_string()`) and `vfs.stat`s each standalone image, per message** — cost that scales with document size and image count on every single update, not just the ones that touch content or images.

- [ ] **Image transmit runs synchronously on the main loop**: `fit_and_encode` plus a blocking tty `write_all`/`flush` plus `terminal.clear()`, all inline with message handling — nothing yields the loop while a large image is being sent.

- [ ] **`runtime::run`'s batch drain has no test harness at all.** The fuzz driver pumps `app::update` one message at a time and cannot model batching, so the drain-and-apply-serially behavior above is currently unverified by any test.

- [ ] **A large document wedges startup at 100% CPU with no partial render and no progress feedback.** Observed: a 22 MB markdown file never reached the message loop; the editor shows nothing until its first message arrives.

- [ ] **Reading view has thin on-screen feedback.** Entering or leaving reading view posts no status message — only the `ReadOnly::Always` refusal does — and there is no read-only indicator anywhere in the title, breadcrumb or tab strip; the sole affordance is the footer hint. That hint is the last entry in `GLOBAL_BINDINGS`, so it is the first global hint `footer_hints.rs`'s greedy prefix-walk truncation drops: summing the hint-entry widths for Editor focus, `⌃P reading` sits at cumulative width 105 cells, and `footer::draw` reserves 11 cells for the `Ln 1, Col 1` readout, so the hint needs a terminal at least 116 columns wide to render at all — at the default 80 columns the row stops after `^K hide pane`. Still discoverable through the F1 help document, which lists the whole binding table regardless of width. A status message and a persistent indicator were both considered for this pass and deliberately not added.

- [ ] **`crates/rune-tui/tests/chrome.rs` was not updated for the reading-view chord, though the implementation plan predicted it would need to be.** The plan called out its two width-sensitive tests, `default_footer_hints_omit_the_aliased_quit_chord` and `footer_global_tail_survives_truncation_with_explorer_focused`, as needing changes once the new binding lengthened the hint row — "expected, not flake". They were not touched, and they still pass, because both assert via `contains` rather than a fixed width or hint count, so the row lengthening the plan anticipated is invisible to them. Recorded so the next person doesn't go looking for a missing change that was never needed.

- [ ] **The `^N new` hint (issue #20) pushes `^E messages` off the footer at width 120.** `truncated_default_hint_spans` drops the first hint that no longer fits AND everything after it; inserting `NewDocument`'s row into `GLOBAL_BINDINGS` ahead of the digit/reading/merge/messages rows shifted every later hint's cumulative width, and `messages` (the table's last row) is now the one that falls off at a 120-column terminal — one column short of where the reading-view hint above already documented the same class of overflow at width 116/80. Deliberately not fixed here: shortening the "new" label or reordering `GLOBAL_BINDINGS` would just move the same fundamental problem (the hint row's total width already exceeds common terminal widths) onto a different row, and the row's max-width budgeting has no test coverage of its own to fix against. `crates/rune-tui/tests/chrome.rs::footer_survives_truncation_with_new_document_hint_at_width_120` was written to assert both hints and pins the measured, real truncation point instead — it currently only asserts `new` at width 120, with this entry as the record of what the stronger assertion (also asserting `messages`) found before it was narrowed. Still discoverable through the F1 help document at any width, same mitigation the reading-view entry above already gives.

- [ ] **`crates/rune-db/tests/multiprocess` — `scenarios::two_stores_closing_simultaneously_surface_no_error_despite_truncate_contention` is a known flake under machine load.** Observed timing out after 30s waiting for child-process marker files (`crates/rune-db/tests/multiprocess/support.rs`'s marker-wait helper) while three cargo builds were running concurrently; passes reliably in isolation. The helper paces multi-process startup on a wall-clock timeout — never order or pace events with wall-clock sleeps, especially in tests — so it will flake again under load rather than being an actual regression. A real fix replaces the wall-clock wait with a deterministic readiness signal. Second sighting 2026-08-05: `scenarios::four_children_append_storm_one_doc_each_all_ack_ok_with_exact_event_counts` timed out the same 30s marker wait during a full `make test` under load and passed in isolation — same helper, same class.

- [ ] **The fuzz harness never gives a document a real `last_sync`, so `MergeState::Active` is unreachable in a fuzz session and all four merge invariants are vacuous.** The session fuzzer builds its `App` fixtures in memory with no store wired behind them, and `merge::begin`'s own fast-path gate refuses outright without a `Some(SyncKind::DiskAhead | SyncKind::Diverged)` `last_sync` — nothing in the generator/driver ever sets one, so `^M` always refuses and the resolver never activates under fuzzing. `MERGE-KEY-FEEDBACK`, `MERGE-SAVE-BLOCKED`, `MERGE-DOC-ACTIVE`, and `MERGE-TITLE-CLEARED` (`crates/rune-fuzz/src/invariant/merge.rs`) all early-return `None` on `!prev.merge_active`/`!prev.merge_pending`, so they currently pass by construction, not by coverage. A real fix needs a store-backed fuzz session — a `Store` behind the fixture `App`, seeded with a divergent disk fact — not a synthetic `last_sync` poke, since the resolver's own landing path (`merge/landing.rs`) reads real blob bytes back through it.

- [ ] **The probe stat short-circuit can miss an external rewrite that preserves size, mtime, and inode within filesystem timestamp granularity.** `rune-db`'s probe classifies a document `Clean` without rehashing when the fresh `stat` matches the last recorded one exactly — the fast path the sync classifier depends on to avoid rehashing every document on every tab switch. An external editor that rewrites a file in place fast enough to land inside one mtime tick, ending up with the same size, can produce a stat identity indistinguishable from "nothing changed" even though the bytes did. Accepted risk, not fixed here: the save-time CAS check (compare-and-swap against the last observed hash before publishing) still catches it at the one point that actually matters — a stale in-memory buffer can never overwrite bytes it didn't know about — so the gap is a delayed *notification* (the disk-changed hint/merge offer may not fire promptly), never a silent data-loss path.

- [ ] **Closing a clean-by-content document with a save still in flight drops its `db_ops` entry, so the eventual materialize ack finds nothing waiting when it lands.** `close_now` sweeps every `db_ops` entry tagged with the closing document unconditionally, whether or not a save enqueued for it is still outstanding — pre-existing with plain `^w`, and now also reachable through the tab-cap eviction in `opentabs::limit::ensure_room` (a clean victim can still have a save racing in the background at the moment it's picked for closure). No observed data loss — the write already committed to disk by the time the ack would have landed, only the bookkeeping ack is orphaned — but sweeping pending-materialize state on close is unowned: nothing currently decides whether an in-flight save should block the close, outlive it, or be cancelled.

## Markdown: <selection>+Cmd+b->Bold +i->italic +`-`-> strikethrough

## Lists

Auto Continue Lists

- blah blah <enter>
- <the cursor>

* lists support

full markdown

### Image rendering — three known hazards

Found while chasing a full UI freeze on opening a large `.gif` (1920x1080).
The freeze itself is fixed — a first transmit no longer forces a redraw — but
the fix is narrow and these three remain.

- [ ] **A terminal clear issued from a decode reply can block the main thread
  forever.** `Effects::force_redraw` calls `Terminal::clear()`, and that call
  was observed never to return: the marker immediately before it logged, the
  one immediately after never did, across four instrumented runs. Input stops
  being processed, the frame never repaints (so the info card sits on
  `decoding...` indefinitely), and `^C`/`^W` do nothing — only an external kill
  ends it.

  Why is NOT understood. `TerminaBackend::clear` is a four-byte `ESC[2J` plus a
  flush, and `write_raw` pushed 785 KB through the same descriptor milliseconds
  earlier. Backpressure was ruled out by moving the clear ahead of the payload:
  it still hung, with nothing in flight. Current guess — unproven — is
  contention between the writer and the parked event-reader thread inside
  `termina`.

  Only the first-transmit path was fixed (it never needed the clear: the diff
  already sees the info card become placeholder cells). **`resize_refit` still
  sets `force_redraw` on a retransmit, so the same hang is theoretically
  reachable on resize.** A retransmit genuinely does need the diff invalidated,
  because its placeholder cells can be byte-identical while the pixels behind
  them changed — but `Terminal::clear()` both writes to the terminal AND
  invalidates ratatui's diff buffer, and only the second is wanted. Worth
  finding a way to get the invalidation without the write.

- [ ] **`fit_and_encode` runs on the main thread.** `handle_image_decoded`
  resizes, PNG-encodes, base64s and chunks the whole image inline — measured at
  ~50 ms release / ~700 ms debug for 1920x1080, and it scales with image size.
  The decode was deliberately moved off-thread for exactly this reason; the
  encode never was. It should move into the decode `Cmd` (or a second one) so
  the reply carries ready-to-write bytes.

- [ ] **Nothing bounds the transmitted payload.** A 2250x1500 image produces a
  4.8 MB APC sequence; 1920x1080 produces 785 KB. All of it is handed to
  `write_raw` in one synchronous call on the main thread. There is no size cap,
  no chunk-count cap and no deadline anywhere in `rune-image` or
  `rune-tui/src/graphics` — verified by sweep. A slow or wedged terminal turns
  that into an unbounded stall.

### Considered and deliberately rejected

- **Gating async paste behind `app.modal`.** A review pass noted that
  `Msg::Paste`/`Msg::ClipboardRead` skip the modal capture that stage 1 of the
  key pipeline applies, so a paste arriving while an Error or Guard is up
  lands in the buffer behind it. Tried it; the session fuzzer rejected it
  within a thousand cases (`PASTE-VERBATIM`, repro: type, `^c` to raise the
  unsaved-changes Guard, paste). The invariant is right and the change was
  wrong: a paste carries user content, and dropping it because a prompt
  happens to be up discards something the user explicitly asked to insert,
  whereas landing it in a journaled, undoable buffer is recoverable. Both
  paste arms now say so in place. Do not re-attempt without first deciding
  what happens to the discarded clipboard text.

## File-size budget

- [ ] **`crates/rune-vfs/src/mem.rs` is over the 500-line file budget (536 lines).** The trash-seam work (`Vfs::trash`, `OpKind::Trash`, and its three unit tests) pushed it past the line the file was already sitting close to (493 lines beforehand). Not split out here to keep the seam landing as one small, reviewable diff; a follow-up should pull the `#[cfg(test)] mod tests` block into a sibling `mem/tests.rs` (or an integration test file, matching how the crate's other `Mem` behavior is already covered by `tests/*.rs`), which alone would bring the production code back under budget.

A batch of twelve splits landed: all six `rune-db` sources, `rune-tui`'s
`document.rs`/`explorer.rs`, `tests/opentabs.rs`, and the two worst test
files (`conceal_roundtrip.rs` at 1453 lines and `tests/highlight.rs`).
`explorer.rs` and `opentabs.rs` — the two previously recorded here — are done.
(Correction: this entry used to also list `save.rs` in that landed batch —
it wasn't; `save.rs` was still one file, 515 lines, unsplit, right up until
the quit-guard/dirtiness-rework plan actually split it into `save.rs`
(start/refusal ladder) and `save/materialize.rs` (the store-backed
materialize dance) — see `crates/rune-tui/TODO.md`'s own entry on the
`materialize_ack.rs` overage that same plan left behind.)

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

- [ ] Re-measured wholesale against the tree with `wc -l` (not accreted from
  individual deltas, so this is every file over the ceiling at one moment,
  not a running tally): `rune-fuzz/src/generate/palette.rs` (562),
  `rune-tui/tests/rename_focus.rs` (560, newly over — the focus-chokepoint
  integration entry below only records it as landing "comfortably under the ceiling" at
  merge time; it has grown past it since), `rune-md/src/emit/mod.rs` (558),
  `rune-tui/src/app.rs` (544, down from 550 after a code review dropped a
  stale relocation-history comment), `rune-tui/src/rename.rs` (532),
  `rune-syntax/src/wrap/mod.rs` (520), `rune-tui/src/runtime/mod.rs` (517),
  `rune-syntax/src/syntax.rs` (505). `rune-tui/src/document.rs`, previously
  listed here at 501 lines, no longer exists as a single file — it is now
  the `document/` directory (`mod.rs` 459, `sync.rs` 195, `tests.rs` 117),
  every member under the ceiling.
  `rune-tui/tests/rename_bind.rs`, previously listed here at 795 lines, is
  DONE: split into `rename_bind.rs` (373 — focus/typing, the
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
`rune-fuzz/src/generate/palette.rs` crossed it the same way during the
title-editing feature: 468 → 517 for `TITLE_MOTION_KEYS`, the five-entry palette `cluster_chrome`
pairs with `CTRL_R_KEY` so a single generated cluster both parks focus on
the title and exercises one of its own word-motion/selection/undo
bindings. Not split further in the same batch — pulling one five-entry
array into its own file over a 17-line overage would be its own drive-by.

The title-editing focus-chokepoint refactor grew `app.rs` and
`rename.rs` further, both already over budget beforehand: `app.rs` 524 → 588
(the private `focus` field plus its three writers —
`focus_title`/`refocus_title`/`set_focus`/`blur_title` — the whole point of
the change, so splitting them out defeats the "one writer" invariant they
exist to enforce) and `rename.rs` 632 → 663 (`begin`'s six-way refusal
enumeration now returns `Commit` instead of `bool`, each arm's reasoning
spelled out). Integrating that work alongside `rr`'s own
`rename.rs` split (`rename_bind.rs`/`rename_collision.rs`/`rename_replace.rs`)
pulled `bind_new`/`create_cmd` out into the pre-existing `rename_create.rs`
sibling, landing `rename.rs` at 523 — still over, but the smallest it has
been since that refactor started, and not a candidate for a second split mid-merge
(the `begin`/`apply_outcome`/`replace_confirmed` state-machine drive is one
coherent unit). `app.rs` stays at 538: the four focus methods are the single
chokepoint the refactor exists to create, and splitting them apart would
recreate the multiple-writer hazard this refactor removes. `tests/rename.rs`'s
own eleven new regression tests (including the single-writer ordering
guard) were never written into a 1196-line monolith — this integration
merge relocated them straight into `rr`'s already-split `rename_bind.rs`
(the read-only-title refusal) and a new sibling `rename_focus.rs` (the
other ten), both comfortably under the ceiling.

The reading-view plan grew three files that were all already over budget,
none of them enough to justify a drive-by split: `palette.rs` 548 → 562
(`CTRL_P_KEY`, one more entry in the same chrome palette this list already
records), `app.rs` 616 → 617 and `rename.rs` 530 → 531 (one line each, both
just swapping a `read_only` bool test for the shared
`ReadOnly::refusal_message` chokepoint). Nothing new crossed the ceiling.

A follow-up code review (`ReadOnly::refusal_message` returning `Option`
instead of answering for `No`, plus the `focus_title`/`rename::begin` guard
chokepoint the sentinel fix's own review flagged as still duplicated) grew
`app.rs` further, 617 → 628, for `refuse_if_read_only` — the one new method
both call instead of each running the check-then-status-then-bail sequence
itself. `rename.rs` grew by one line, 531 → 532, swapping its half of the
duplicated guard for a call to it. Neither newly crossed the ceiling.

The single most-deferred item remains `app.rs`'s `handle_key` /
`handle_editor_key` / `handle_db_event` extraction, deferred across nine
consecutive work packages.

The markdown line-decoration work (`LineDecor` model + emit population)
pushed three more files over or further over the ceiling: `rune-md/src/emit/
walk.rs` 515 → 539, since brought back under budget by its own later split
into `walk_inline.rs`; `rune-md/src/emit/mod.rs` 499 → 535 (the new
`emit_with`/`EmitOut::icons`/`EmitOut::decors` plumbing the 3-arg `emit` now
wraps); and `rune-syntax/src/syntax.rs` 499 → 505 (the new
`SyntaxLine::decor` field and its doc comment). New logic went into new
sibling files (`emit/decor.rs`, `emit/decor_tests.rs`, `rune-syntax/src/
decor.rs`); only wire-up lines touched
`emit/mod.rs` and `syntax.rs`, but that was still enough to cross or extend
the ceiling, and neither has been split since.

- [ ] The Explorer type-to-search feature (no wall clock) grew two more
  already-over-budget files: `rune-tui/src/app.rs`
  567 → 577 (`set_focus`'s new blur clear — the one chokepoint every route
  off the Explorer funnels through, so it has to live in the same writer
  the title's own blur clear already does, not a new file) and
  `rune-fuzz/src/generate/palette.rs` 517 → 544 (`EXPLORER_SEARCH_KEYS`,
  the printable-letter palette `cluster_chrome`'s new `^b`-then-type arm
  draws from — a four-entry array, not worth its own file over a 27-line
  overage). New logic otherwise went into the new sibling `explorer_
  search.rs` (422 lines, under budget), which is also why `explorer.rs`
  itself (499) and `explorer_keys.rs` (260) stayed under the ceiling
  despite gaining the feature's state and dispatch wiring.
- [ ] `rune-db/src/writer.rs`, previously recorded here at 497 lines and about to breach, has since been split and is now 356. `rune-db/src/materialize.rs` is still within a few lines of the ceiling at 496 — whoever touches it next should take the split rather than squeeze under.
- [ ] `rune-md/src/emit/mod.rs` 536 → 558 during the code-pipeline unification (the `base_scope` parameter threaded through `emit_with`/`fill_gaps`, plus its rationale comments). It was already over the ceiling before that change, so this is a deepening rather than a breach. The natural split is the gap-fill and span-ordering pass — `fill_gaps` plus the buffer-order re-sort it exists to preserve — into an `emit/gaps.rs` sibling; `emit_with` itself is the orchestration and should stay.
- [ ] The `rune-db` splits copy their test scaffolding rather than share it — `open()`, `insert_test_document`, `Fixture`, `always_dead` and friends are now verbatim in both `rename_bind.rs` and `rename_replace.rs` (~50 lines), and similarly across the `writer_*`/`store_*` pairs. Note this predates the splits as a crate-wide habit (`open()` alone is defined in sixteen files), so the fix is one `#[cfg(test)]` support module for the whole crate — the pattern `conceal_common`/`opentabs_common`/`highlight_common` already use on the test side — not a per-split patch.
- [ ] The Explorer live-preview feature grew two more already-over-budget
  files by a handful of lines each: `rune-tui/src/app.rs` 544 → 555 (`update`'s
  new `focus_before`/`on_focus_changed` diff, alongside the existing
  `active_before`/`buffer_version_before` ones it's modelled on — the whole
  point is that this is the ONE place a whole-message before/after diff
  belongs, so splitting it out would recreate the multiple-chokepoint hazard
  the existing pattern avoids) and `rune-tui/src/runtime/mod.rs` 517 → 555
  (`read_preview_cmd` plus its `MAX_PREVIEW_BYTES` constant — the `Cmd`
  constructors' own home, alongside `read_file_cmd`/`load_dir_cmd`). All the
  new state/lifecycle logic itself landed in a new `explorer_preview/`
  directory module (`mod.rs` 272, `tests.rs` 381, both comfortably under
  budget) rather than either of these two, which is why they only grew by
  wiring, not by feature weight.
- [ ] The merge feature's fuzz work grew `rune-fuzz/src/generate/palette.rs` further, 562 → 609 (`MERGE_KEY`, `MERGE_RESOLVE_KEYS`, and their doc comments — the merge chord's own entries in the same static palette every other `cluster_*` strategy already draws from). Already over the ceiling before this change, so a deepening, not a breach; not split further in the same pass — pulling one const and one five-entry array into their own file over the same kind of small overage the file's own history already declines to split for (see the `EXPLORER_SEARCH_KEYS`/`TITLE_MOTION_KEYS` entries above) would be its own drive-by.
- [ ] The merge feature as a whole pushed `rune-tui/src/app.rs` over the ceiling again, 544 → 553: `pub merge: crate::merge::MergeState` plus `next_merge_gen` (the merge attempt's own generation counter, minted the same way `next_rename_gen` already is) and their `Default` initializers. Both are single fields on the one `App` struct every pane's state already lives on — splitting them out would mean either a second `App`-adjacent struct with its own borrow story or threading a new parameter through every merge call site, neither of which the nine-lines-of-wiring this needed would justify.
- [ ] The message-log feature newly pushed `rune-tui/src/global.rs` over the ceiling, 468 → 504: `GlobalCommand::ToggleMessages` plus its `^E`/`⌘E` rows (mirroring every other focus-command pair already in `GLOBAL_BINDINGS`) and a `global_e_binding_is_not_already_bound_in_any_pane_table` cross-table guard test matching the file's own `global_p_binding_...`/`global_m_binding_...` precedent. Not split out over a 4-line overage — the binding table and its guard tests are one coherent unit, the same reasoning the `EXPLORER_SEARCH_KEYS`/`TITLE_MOTION_KEYS` entries above already give for `palette.rs`. Also deepened the two already-recorded chronic breaches above it in this same feature: `rune-tui/src/app.rs` (`guard: Option<GuardPrompt>` and `messages: MessageLog` replacing the deleted `modal: Option<Modal>` field) and `rune-tui/src/rename.rs`/`rune-tui/src/layout.rs` (`banner::`/`Modal::` renamed to `guard::`/`messages::` in place, net neutral to a couple of lines).
- [ ] The in-file search feature as a whole deepened three already-recorded breaches. `rune-tui/src/layout.rs` 562 → 614: `geometry`'s new bar-row reservation (one extra rect carved between the title row and the editor while `app.search.is_some()`) plus its own test coverage. Split candidate unchanged from the existing entry above — pull `geometry`'s rect-building and its assertions apart from `resolve`'s mode dispatch. `rune-tui/src/global.rs` 540 → 611: the `ToggleSearch`/`SearchNext`/`SearchPrev` rows and their doc comments, plus the `claimants`-helper extraction performed alongside them to fold three near-duplicate cross-table guard tests (that extraction is a net line reduction on its own, but the six-table `MERGE_BINDINGS` widening and the new commands' own guard tests outweigh it). Split candidate: the `#[cfg(test)] mod tests` block is roughly half the file and self-contained behind `use super::*;` — move it to a sibling `global/tests.rs`, the same split `search/mod.rs`/`search/keys.rs` already use. `rune-tui/src/app.rs` 532 → 565: `pub(crate) search: Option<search::SearchState>`, `last_search_query`, `search_history_ops` — three fields on the one struct every pane's state already lives on, the same reasoning the merge/message-log entries above give for not splitting `App` itself over new fields.
- [ ] The search fix round (paste routing, blur-on-focus-move, nav revalidation) pushed `rune-tui/src/pane.rs` newly over the ceiling, 466 → 544: the hoisted bar-close-on-focus-move chokepoint (every focus-moving global command now funnels through one close-then-blur sequence instead of each pane duplicating it), the merge-mode search refusal, and the closed-bar next/prev dispatch with its no-previous-query feedback. Split candidate: `handle_global_command`'s per-command match arms are self-contained behind `App` and `Effects` — move them (or just the search/merge arms) to a sibling `pane/global_commands.rs`, leaving the pane focus-routing table in `pane.rs`. Also deepened two recorded breaches above: `rune-tui/src/runtime/mod.rs` 612 → 618 (`PasteTarget::Search` and its doc comment on the existing clipboard-read plumbing) and `rune-tui/src/app.rs` 565 → 573 (`last_persisted_search_query`, the debounce fact the persist-on-close write keys off).
- [ ] The message-log feature's auto-collapse work deepened the already-recorded `rune-tui/src/runtime/mod.rs` breach, 555 → 572: `Msg::MessagesCollapseTimeout { generation }` and `CmdKind::MessagesCollapseTimeout`, the pane's 5s auto-collapse timer's transport — the same doc-agnostic, generation-tagged shape `Msg::ConfirmTimeout`/`Msg::SaveConfirmTimeout` already use, so it belongs beside them rather than in a new file for one variant pair — plus a later `Msg::Warning` variant and its doc comments.
- [ ] Merging the auto-collapse and mouse selection/copy work into `rune-tui/src/messages/mod.rs` pushed it over the ceiling, 299/444 on the two branches alone → 534 merged: neither work package crossed the line by itself, but their disjoint additions (the `armed`/`generation` timer state and accessors; the `mouse`/`copy_selection`/hit-testing plumbing) landed in the same file since both extend the same `MessageLog`/pane API. Split candidate: pull the mouse handling (`mouse`, `mouse_down`, `mouse_drag`, `copy_selection`, `relative`, `pane_rect`) into a sibling `messages/mouse.rs`, mirroring the existing `messages/render.rs` split — not done in this merge to stay a pure integration. A code-review repair round further deepened this, 534 → 540: the tail-pinning `pinned` field/doc comments (a newly-posted message must stay visible instead of scrolling off-screen) and the C1 control range added to `sanitize`'s filter — both single-field/single-clause fixes with no natural home outside this file's own state and sanitizer, so not their own split. `is_copy_chord` (named in the split candidate above) no longer exists — the same round replaced it with a lookup through `keymap::resolve` against `EDITOR_BINDINGS`, the crate's one source of truth for the Copy chord, removing a few lines rather than adding a split motivation.
- [ ] Integrating the message-log branch into the `^M`-rebind branch deepened `rune-tui/src/global.rs` further, 504 → 540: the two branches independently extended the SAME `Merge` variant/binding row from disjoint sides (message-log's own growth, entry above, plus the unrelated `^M`-rebind's expanded `Merge` doc comment explaining why only `^M` binds and its two new cross-table guard tests pinning that `⌘M` binds nothing). Neither branch alone crossed the ceiling growing this row; combining them did. Split candidate: none of the individual additions (one doc comment, one test) is large enough to justify its own file over a binding table this small — a future pass that splits `GLOBAL_BINDINGS`'s doc comments out from the table itself (mirroring `footer_hints.rs`'s doc/data split) would shrink every entry in this file at once rather than just this one.
- [ ] The trash feature (issue #19) deepened three already-over-budget files further: `rune-tui/src/global.rs` 540 → 609 (`GlobalCommand::Trash`, its `⌘⌫`/`^⌫` `GLOBAL_BINDINGS` rows, and the `global_backspace_chords_are_not_already_bound_in_any_pane_table` cross-table guard test — the same binding-table-plus-guard-test unit the message-log and `^M`-rebind entries above already decline to split for), `rune-tui/src/runtime/mod.rs` 572 → 586 (`CmdKind::Trash` and `Msg::TrashDone { generation, path, result }` — the transport pair every other async `Cmd`/`Msg` round-trip in this file already lives beside), and `rune-fuzz/src/generate/palette.rs` 611 → 625 (`TRASH_KEY`, one more chord constant in the same static palette this list already records growing repeatedly). None split out here for the same reason each prior entry gives: a single variant/row/constant pair over an already-breached ceiling isn't its own file. Split candidates, if anyone picks this up: `global.rs`'s doc-comment/table split (named above) would shrink every entry at once; `runtime/mod.rs`'s `Cmd`/`Msg` pairs could move to a `runtime/messages.rs` sibling; `palette.rs`'s chord constants could move to a `palette/keys.rs` sibling, leaving `TYPE_PALETTE`/`MARKDOWN_FRAGMENTS` behind.
- [ ] `rune-fuzz/src/generate/cluster.rs` was already over the 500-line ceiling (516 lines) before the trash feature touched it, undocumented until now; the feature's own `cluster_chrome` arm (`Just(vec![Action::Key(TRASH_KEY)])`) added one line, 516 → 517. Not split here — a one-line deepening of a pre-existing, previously-unrecorded breach isn't the moment to take on a `cluster_chrome` split; a future pass should pull the strategy functions (`cluster_chrome`, `cluster_highlight`, etc.) that this file has accreted one at a time into per-strategy siblings.
- [ ] The trash code-review repair round (single-flight enforcement, F1) deepened `rune-tui/src/app.rs` further, 538 → 546: `pub(crate) trash_pending: Option<PathBuf>` plus its `Default` initializer — the one field `trash::request_trash`/`trash::confirm` both gate on, the same single-slot shape `rename::RenameState`'s ticket-based `in_flight()` already uses for the symmetric rename case. Not split out over an 8-line overage — one field and one initializer on the struct every other pane/doc/op piece of state already lives on, for the same reasons the merge/message-log/trash entries above already give.
- [ ] The `^N`/`⌘N` new-document chord (issue #20) deepened `rune-tui/src/global.rs` further, 540 → 590: `GlobalCommand::NewDocument` plus its paired binding rows, a `global_n_binding_is_not_already_bound_in_any_pane_table` cross-table guard test matching the file's own `global_p_binding_...`/`global_m_binding_...`/`global_e_binding_...` precedent, and hoisting the three existing tests' duplicated nested `claimants` helper into one shared module-level function (a net reduction on its own, more than offset by the new binding rows and guard test). Split candidate unchanged from the entry above: pulling `GLOBAL_BINDINGS`'s doc comments out from the table itself (mirroring `footer_hints.rs`'s doc/data split) would shrink every entry in this file at once rather than just this one.
- [ ] Re-measured `wc -l` against the tree at the tab-cap plan's own merge (correcting the "Re-measured wholesale" snapshot above, whose 544 for `rune-tui/src/app.rs` is stale): `rune-tui/src/app.rs` is now 536, still over the ceiling but net down since that snapshot — the pin toggle/MRU/tab-cap work added no new `App` fields, only call-site wiring. `rune-tui/src/global.rs`, missing from that same snapshot (it hadn't crossed the ceiling yet at the time), is now 600 — every entry above this one recorded a small deepening of the SAME `GLOBAL_BINDINGS` table/guard-test pair, and none of them was ever split out; the table itself (`GLOBAL_BINDINGS` plus its per-row doc comments) is now the single largest piece of the file. Split candidate: move the binding-collision guard tests (`global_p_binding_...`/`global_m_binding_...`/`global_e_binding_...` and siblings) out into `crates/rune-tui/tests/global_bindings.rs` — they exercise `GLOBAL_BINDINGS` purely through its public shape and need nothing crate-internal, unlike the dispatch logic the rest of the file holds. Neither the tab-cap plan's own `limit.rs` (436, new — the review-fix merges' `ensure_room` hardening plus the union of both sides' new tests) nor any other file it touched (`opentabs/mod.rs` 444, `pane.rs` 424, `workspace/mod.rs` 490 — up from 468 after `resolve_and_read`'s read-before-evict extraction, still short of the ceiling, `workspace/close.rs` 353, `document/mod.rs` 481, `explorer_preview/mod.rs` 322, `explorer_keys.rs` 294, `footer.rs` 429) crossed the ceiling — those measurements were taken on the tab-cap branch itself, before this merge folded it into `rr` alongside the in-file-search/`^N` work recorded above; see the entry below for the post-merge counts.
- [ ] Merging the tab-cap plan (issue #21) into `rr` collided two independently-built chords on the same key: the tab-cap plan bound `GlobalCommand::TogglePin` to `^g` (verified unclaimed at the time it was built), but the in-file-search feature had since claimed `^g`/`⌘g` for `SearchNext` on `rr`. Resolved by re-keying `TogglePin` to `^j` (also unclaimed, confirmed by the same `assert_unclaimed_by_any_pane_table` cross-table guard, now covering `MERGE_BINDINGS` too — the pin toggle's own ad-hoc guard test had reimplemented the shared helper by hand and missed that table, folded into the shared one here). Re-measured `wc -l` post-merge: `rune-tui/src/global.rs` 646 → 748 (the tab-cap plan's own `TogglePin` variant/binding/guard test, deepened further by the `^g`→`^j` rekey's explanatory comments), `rune-tui/src/pane.rs` 569 → 571, `rune-tui/src/app.rs` 572 → 590, `rune-tui/src/document/mod.rs` 491 → 495 (`pinned: bool`) — all four already chronically recorded above, no new split motivation from this merge alone. `opentabs/mod.rs` (444), `opentabs/limit.rs` (436), `workspace/mod.rs` (490), `workspace/close.rs` (353) stayed under the ceiling.

## Parked tickets

Groomed, verified against the tree, and deliberately not worked — each needs its own plan.

(Both title tickets that used to be parked here are done — see "Recently closed".)

## Recently closed

- **The title field is a real text editor, and the file extension is editable.**
  Closes both parked title tickets at once. `rune-tui/src/field.rs`'s `TextField`
  is the reusable editable core the verbatim-editor ticket asked for — buffer,
  one cursor with an anchor, and its own in-memory `Journal` that never reaches
  the recovery store — and the title resolves keys through the existing
  `EDITOR_BINDINGS`, so ⌥-arrows, ⇧-selection, ⌘A, ⌘Z/⇧⌘Z and ⌘C/⌘X/⌘V all work
  on a file name. Two deliberate divergences from the tickets as groomed: the
  title keeps its **own** undo journal (the ticket recommended ⌘Z undo the
  document instead), and stem and extension are **one buffer with a derived
  split** rather than two tracked strings — the latter forced by the
  requirement that the dot itself be editable, so `lessrc.md` can become
  `lessrc`. Losing focus, not Enter, is the single chokepoint that commits the
  rename; Enter and Down merely cause the focus loss.

- **Zero-width edit batches no longer dirty a clean file** — the commit chokepoint drops edits that change nothing.
- **`build_5k_doc` has one source of truth** — the bench and perf-guard copies had already silently diverged (the guard measured a table-bearing document the bench did not).
- **`Mem::stat` and `read_dir` share one synthetic-directory predicate.**
- **Span-cap truncation surfaces a status line**, with timeout outranking it when both hold.
- **`make test-fuzz` no longer dirties a tracked file** — proptest persistence moved under the gitignored artifacts directory, so the gate list is idempotent.
- **`db_ops`/`db_load_versions` merged into one `PendingOp`** — the two maps were swept separately, leaking load versions on document close; one value makes that unreachable.
- **Rename displaced-bytes attribution audited: NOT reachable.** The observation is attributed to the renaming document and carries `origin='swap'`, which ancestor selection's `origin IN ('load','save','resolve')` filter excludes outright. Pinned by a test seeded to fail if either guard were removed.
- **Two of the three comrak strict-invariants repros are closed** by the lone-`\r` shadow-copy parse, re-verified under the feature flag. The third still panics, isolating the surviving cause to tab-stop expansion alone.
- **`crates/rune-tui/src/search/keys.rs` is back under the 500-line budget.** By the time the hardening pass reached it, the nav and history merges had grown it to 813 lines. Rather than the history-browsing extraction this entry used to recommend, the test module (`mod tests`, 515 lines) moved to a sibling `search/keys/tests.rs`, the same split `search/tests.rs` already uses for `search/mod.rs` — `keys.rs` itself is now 297 lines, all keystroke-handling logic still in one place.

## Closed without action

- **`ticket-hide-cursor-unfocused-editor.md`** — the end goal already holds. `Document::shows_caret()` is the single predicate, `apply_cursor_overlays` early-returns on it (gating the caret *and* the selection highlight together), the fuzz snapshot carries `caret_visible`, the `CUR-NO-CARET-HIDDEN` invariant fails any REVERSED cell rendered while the caret is hidden, and `crates/rune-tui/tests/tui_render_focus.rs` pins all three cases.
