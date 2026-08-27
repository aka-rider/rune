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

#### `⌘⌫` / `^⌫` are globally bound to Trash, shadowing delete-to-line-start in every field and the editor
- **Where**: `crates/rune-tui/src/global.rs:217-228` + `crates/rune-tui/src/dispatch.rs:271-274` (global table consulted before any focus routing); layering issue at `crates/rune-tui/src/pane_bar_policy.rs:30-37`.
- **Wrong**: on macOS `⌘⌫` means "delete to start of line" and `⌥⌫`/`^⌫` word-delete in every text field. Here they reach `GlobalCommand::Trash` from the editor, title field, search bar, finder and palette alike — no field sees them. In the title field the hoisted `blur_title` runs first, so `⌘⌫` *commits the in-progress rename* and then raises a Trash prompt for the file; a `y/N` guard is the only thing between muscle memory and the Trash. With the palette open, `bar_policy(Trash)==LeaveOpen` raises the trash confirmation *underneath* the still-painted palette overlay, violating "modal capture is total".
- **Instead**: do not bind a destructive command to a chord that is a standard text-editing key; require an unambiguous Trash chord, and let fields consume backspace-family chords first.
- **Confidence**: confirmed.

### CORRECTNESS

#### Trash has no mutual exclusion with an in-flight rename
- **Where**: `crates/rune-tui/src/trash.rs:25-47,81-94` (no `rename.in_flight()` check); contrast `crates/rune-tui/src/save/gate.rs:41-44` and `rename.rs:210-213`, the documented symmetric save/rename pair.
- **Wrong**: ^R, type name, Enter (rename enqueued, ack async); before it lands, `⌘⌫` reads the still-old `file_path` and raises a Trash guard for the old file; `y` spawns `trash_cmd(old_path)` while `rename_excl(old→new)` is in flight on the same inode. Whichever loses reports a confusing failure. No bytes lost (both atomic), but two destructive ops race with no refusal, in a codebase that refuses the save/rename pair for exactly this reason.
- **Instead**: refuse trash while a rename is in flight (mirror the save gate).
- **Confidence**: confirmed.

#### Any left-click while the finder is open cancels it — including a click on a result row
- **Where**: `crates/rune-tui/src/commands/mouse.rs:97-100`.
- **Wrong**: `Down(Left)` + `filesearch().is_some()` → unconditional `filesearch::cancel`, with no rect-containment test and a `return` before the finder mouse-routing arm at `:126`. The palette arm right below does test containment. Clicking the row you want discards the query; the only feedback is the overlay vanishing.
- **Instead**: containment-test like the palette arm; route clicks inside the finder rect to its rows.
- **Confidence**: confirmed.

#### Preview reply and real file-open reply are indistinguishable — the real open is swallowed and its anchor lost
- **Where**: `crates/rune-tui/src/explorer_preview/mod.rs:53-89`, consumed at `crates/rune-tui/src/workspace/mod.rs:116-118`.
- **Wrong**: `read_preview_cmd` and `read_file_cmd` both reply `Msg::FileOpened`, correlated by path only, then by re-deriving `is_current_target` from the live cursor at arrival — exactly what the constitution forbids ("killed by a generation/version echo on the request, never by resolving live state on arrival"). If the explorer cursor sits on `notes.md` with a preview in flight and the user follows a link to `notes.md#section`, the link reply is consumed as the preview's, `handle_file_opened` returns early, the anchor is dropped and focus never moves — the user lands at the top of a preview tab with no explanation.
- **Instead**: carry a generation/purpose on the preview request and match on it, as the explorer dir-loads already do.
- **Confidence**: plausible (needs the race window).

#### Tab-limit eviction hijacks the active document and never says the requested open was refused
- **Where**: `crates/rune-tui/src/opentabs/limit.rs:51-64` + `crates/rune-tui/src/workspace/mod.rs:262-264`.
- **Wrong**: with the tab limit reached and every eligible tab dirty, `ensure_room` calls `switch_to(victim)` — moving the user to a doc they weren't looking at — then arms a `DirtyClose` guard and returns false *before* the "Tab limit reached" warn; the caller (e.g. `toggle_help`) then bare-returns. The user pressed F1, got teleported to an unrelated tab with a close prompt, and was never told Help was refused. Data safety itself is fine (pinned/preview/saving/pathless excluded, guard always armed on a dirty victim).
- **Instead**: refuse the open with feedback *without* switching the active document, or only switch after the victim is actually closed.
- **Confidence**: confirmed.

#### Bracketed paste while a non-editor pane is focused inserts into the editor document invisibly
- **Where**: `crates/rune-tui/src/commands/clipboard.rs:166-175`.
- **Wrong**: `^E` focuses `Pane::Messages`; a terminal ⌘V routes `handle_paste_content(app, app.active, text)` into the *editor's* document at a caret that isn't even painted (`app_view` clears `focused` when focus≠Editor). Journaled/undoable, so not data loss, but an unannounced insertion into the user's words at an invisible point. The message pane is `ReadOnly::Always` and should refuse, not redirect.
- **Instead**: route paste to the focused pane; refuse (with feedback) when it is read-only.
- **Confidence**: confirmed.

#### Recents and the workspace walk race, so a file can be listed twice
- **Where**: `crates/rune-tui/src/filesearch/mod.rs:167-181` vs `:275-290`.
- **Wrong**: both Cmds run on their own threads, replies unordered. `handle_scanned` builds its dedup `seen` from `state.recents` at reply time; `handle_recents_loaded` only assigns `mru_rank` and never re-filters `walk`. Scan-lands-first → duplicate rows and a doubled `matched/total` count. The existing test pins only the recents-first ordering.
- **Instead**: dedup at render time from the merged set regardless of arrival order.
- **Confidence**: plausible.

#### Multi-cursor uppercase/lowercase refuses with "edit failed" and does nothing
- **Where**: `crates/rune-tui/src/commands/case.rs:33-57`; root cause at `crates/rune-tui/src/commands/edit_core.rs:92-98,197-199`.
- **Wrong**: two cursors inside the same word (e.g. alt-click at byte 2 and byte 3 of `"hello world"` — `CursorSet::merge` leaves them separate since `2 >= 3` is false) both resolve `word_range_at` to `(0,5)`, so `per_cursor_selection_edits` builds two *identical* `Edit{0,5,"HELLO"}`. `coalesce_touching_edits` merges only when both inserts are empty, so both survive; `SortedEdits::sort` does not check overlap (only `validate` does), so `build_edited_content`'s second pass calls `content.get(5..0)` → `OutOfBounds`. Executed: content unchanged, log `edit failed: edit out of bounds: [5,0) len=11`, journal length 0. `edit_core.rs`'s own doc names this class but the fix was scoped to pure deletions. Distinct from the undo-path entry above and from the forward-batch undo entry: this is the *forward* path with two identical non-delete edits, reachable from a live keystroke.
- **Instead**: dedup/coalesce overlapping identical per-cursor edits before `apply_edits` (a word-range case change from two cursors in one word is one edit), or validate-and-drop overlaps rather than reaching `build_edited_content`.
- **Confidence**: confirmed (executed).

#### Kitty image IDs collide across documents — wrong image shown / another document's image deleted
- **Where**: `crates/rune-tui/src/workspace/mod.rs:184` (`alloc_id`, no probing), `crates/rune-tui/src/graphics/embed/alloc.rs:15-26` (per-document allocator), `crates/rune-image/src/ids.rs:37-44` (FNV-1a truncated to 24 bits).
- **Wrong**: the embed allocator only deconflicts *within* one document; whole-document image IDs bypass it, and Kitty IDs are terminal-global. FNV is trivially invertible, so a hostile vault can name `a.png`/`b.png` to collide at 24 bits: opening both notes makes one overwrite the terminal's data for that ID (the other renders its pixels), and `despawn_gone` emits `encode_delete(id)` that blanks the other document's image.
- **Instead**: allocate whole-document IDs through a terminal-global probing allocator, not a content hash.
- **Confidence**: confirmed (mechanism).

#### `Document::hydrate` journals crash-recovery adoption with empty cursor sets
- **Where**: `crates/rune-tui/src/document/mod.rs:287-292` — `cursors_before: Vec::new(), cursors_after: Vec::new()`.
- **Wrong**: after crash recovery the first ⌘Z undoes the hydration and, because `cursors_before` is empty, restores a cursor set built from nothing. The empty cursor vectors are clearly wrong even if the buffer revert is intended.
- **Instead**: record the real cursor sets on the hydration step.
- **Confidence**: plausible.

### MINOR

- **Unbounded `rune_vfs::get(path, None)` cluster** (`max_bytes: None` disables the 64 MiB gate). Beyond the image-open OOM above, three more sites read attacker-growable files whole into memory: `crates/rune-vfs/src/publish.rs:39` (`current_sighting`, the *worst* placement — the CAS read during a save, buffer unsaved), `crates/rune-tui/src/filesearch/walk.rs:113` (every `.gitignore`-family file in the scan, then copied again by `String::from_utf8`), and `crates/rune-db/src/bracket.rs:35` (probe re-read). Make `max_bytes` non-optional so each caller must name a limit.
- **Read-only edit refusal is completely silent** — `crates/rune-tui/src/commands/edit_core.rs:75-78` returns false with no message and every caller discards it; typing/Backspace/⌘X/⌘V on the Help tab or a reading-mode doc does nothing and says nothing (the palette path *does* explain via `registry/avail.rs`). `editor_exec.rs` even spawns `pbpaste` first and drops the result.
- **Save gate has two silent rungs** — `crates/rune-tui/src/save/gate.rs:31-36`: a missing document and an image document both return `Refused` with no message (⌘S on an image does nothing), while every other rung posts; `pane_command.rs:79` discards the result. Contradicts `guard.rs:328-331`.
- **`settle_pending_materialize` drains and discards every non-`MaterializeVfsDone` message at shutdown** — `crates/rune-tui/src/runtime/exit_settle.rs:29-45`; a last-moment `SaveDone(Err)` is swallowed, so the user quits believing a failed direct save landed. Loses the report, not bytes.
- **`close_now` cancels the rename's feedback, not the rename** — `crates/rune-tui/src/workspace/close.rs:89,97`; the file is still renamed on disk (or, in the `Collided` case, not renamed) and the user is never told.
- **No-store draft create leaves the document permanently dirty** — `crates/rune-tui/src/rename_create.rs:98-136`→`rename.rs:434-443`; `bind_to` binds the path without advancing the saved baseline, so a byte-matching file stays "unsaved" forever and arms a spurious quit guard.
- **Palette/finder swallow `⌘V` (and empty-list `Tab`) with no feedback** — `crates/rune-tui/src/palette/keys.rs:97-101,161-179`; `PasteTarget` has no `Palette` variant.
- **After a trash the explorer selection jumps to `..`** — `crates/rune-tui/src/explorer_dirload.rs:41-55`; a Refresh keeps the vanished row's name so `by_name` misses and the cursor resets to index 0; the same handler's `clear_search` also wipes an in-progress type-to-search on any async refresh.
- **`messages.doc.focused` is cached shadow state** — `crates/rune-tui/src/messages/mod.rs:171,178`; set true in `focus()`, cleared only in `collapse()`, so the pane keeps painting its selection as focused after the editor regains focus, and a stray selection pins the pane open until the next post.
- **Click in the message pane acts on a vetoed focus transition** — `crates/rune-tui/src/messages/mod.rs:355-375`; `mouse_down` hit-tests and latches a drag without re-checking that `focus()` succeeded (both sibling handlers do), so a click with an invalid title in the field still selects text and emits OSC 52 on mouse-up.
- **`^1`–`^0` for a non-open tab moves focus then does nothing** — `crates/rune-tui/src/pane_global.rs:64-67`; `set_focus_pane(Editor)` fires before `select_tab` early-returns on an out-of-range index.
- **Duplicate refusal message leaving an invalid title** — `crates/rune-tui/src/pane_command.rs:49` + `crates/rune-tui/src/focus/mod.rs:295-299`; hoisted `blur_title` posts the refusal, then the arm's `set_focus_pane` re-enters `on_blur` and posts it again; for `^E` the messages pane opens and paints as focused while the title still owns the keyboard.
- **A `markdown` fence's highlight pass ignores its time budget** — `crates/rune-tui/src/runtime/highlight_cmd.rs:58-91`; the `RegionLang::Markdown` arm never uses `left`, so one pathological ```` ```markdown ```` fence runs an unbounded comrak parse on the highlight thread.
- **Inline embeds are never re-fitted on a pane resize** — `crates/rune-tui/src/graphics/resize_refit.rs:7-49` handles only the whole-document image; every inline `![](…)` keeps its stale footprint until an mtime change re-decodes it.
- **Snapshot debounce keys off `active_doc()` across a possible active-document change** — `crates/rune-tui/src/app.rs:227,235-238`; a message that switches tabs arms/omits the snapshot debounce for the wrong document (journaling is unaffected; only the snapshot anchor is mis-timed).
- **`probe`'s deferral is silently lost when the file binding is missing** — `crates/rune-tui/src/db_enqueue_load.rs:144-149` returns whether or not it stashed `pending_probe`, so `last_sync` can stall until an unrelated event probes again.
- **`unreachable!` panics in the update loop** — `crates/rune-tui/src/dispatch.rs:61-63,161-163` (`Msg::Timer`/`RecentsLoaded` illegal pairings) and `crates/rune-vfs/src/testing.rs:20` (non-`cfg(test)`-gated, compiles into the release binary). All unreachable today, but the constitution routes "can't happen" through `assert_invariant!` or an enum shape, never a panic.
- **Latent subtraction underflow in the fixed-indent hint builder** — `crates/rune-md/src/parse/indent.rs:30` (`candidate_end - scan_start`); no current producer overshoots the line, but a sibling test constructs exactly such an overshooting hint elsewhere, and this is the one site in the family that would panic rather than clamp. Use `saturating_sub` or an early continue.
- **Two documents on one file from differing path spellings** — `crates/rune-tui/src/workspace/mod.rs:211-216` compares `file_path` byte-for-byte, so a relative CLI positional and an absolute Explorer open of the same file yield two documents each with its own `db_id`/`FileBinding`; the shared-baseline probe and epoch bump then don't cover the pair. `materialize_ack/reactions.rs:215-224` resolves paths for the same hazard; the tab-dedup chokepoint does not.
- **Tall images repeat their first row past 297 rows** — `crates/rune-tui/src/graphics/footprint.rs:12` leaves `rows` uncapped, so `placeholder.rs:305-309` silently falls back to `DIACRITICS[0]` for row indices ≥ 297; a 1080×9000 screenshot renders its bottom ~36 rows as repeats of its top row, with no truncation reported.
- **`DecorPiece::cells()` measures `first` but is used as the width of `cont`** — `crates/rune-syntax/src/decor.rs:13-15`, consumed at `crates/rune-syntax/src/wrap/decor.rs:92,110` and read as truth at `crates/rune-tui/src/render/decor.rs:31`. `SegDecor.cells` is the number `mouse_hit::offset_at_ordinary` subtracts from a click column and `overlay::apply_cursor_overlays` adds to `visual_col`, but the actual cells on a continuation row are summed from `p.cont` while `cells` came from `p.first`. Any decor whose `cont` differs in display width from `first` would offset every click and caret on a wrapped continuation row. Kept equal today only by discipline in `rune-md/src/emit/decor.rs` — two independent derivations of one number ("no shadow state"). Latent, not reachable today.
- **Diff fold view paints backgrounds from one document's offsets onto merged cells of two** — `crates/rune-tui/src/render/mod.rs:271-289`; in the fold path `augment_fold` interleaves left-document rows with right-document rows, then `paint_backgrounds` is called with the active (right) document's content and right-side region ranges, keyed purely on `Cell::buf_offset` with no document tag — a left-document cell whose offset falls inside a right region gets the wrong background. Cosmetic; no buffer bytes involved.
- **Wrap segment can exceed the width budget when decor is present** — `crates/rune-syntax/src/wrap/mod.rs:147`; the force-include-one-cluster fallback can emit a 2-cell cluster into a 1-cell content budget (`"# 👍🏽"` at width 3). Documented "always make progress" behavior with no byte/coordinate consequence — recorded so it isn't mistaken for a new bug.

## Architecture

### Session-driver migration residue
- **Where**: `crates/rune-tui/tests/rename_common/mod.rs` (kept App-layer fixtures: `seeded_vfs`, `app_with`, `app_with_store`, `unsaved_named_app_with_store`, `next_event`, the `wait_for_*` waits, `send`, `type_text`, `type_new_name`); its consumers are THIRTEEN test binaries, not six: `bind_new_named.rs`, `save_state_machine.rs`, `materialize_dead_writer_reentrancy.rs`, `materialize_fatal_two_docs.rs`, `refused_hydration_detach.rs`, `reading_view.rs`, `rename_gate.rs`, `rename_bind.rs`, `rename_refusals.rs`, `rename_collision.rs`, `rename_replace.rs`, `rename_clipboard.rs`, `rename_focus.rs`; `navhistory_common` (embeds `explorer_common` via `#[path]`, still builds bare `App`s) with consumers `navhistory.rs`/`navhistory_browse.rs`; `set_doc_db_for_test` (consumed by the kept fixtures, `materialize_fatal_two_docs.rs`, and `g7_shared_file_baseline.rs`).
- **Wrong**: the 2026-08-13 migration moved nine `*_common` modules onto `rune_fuzz::Session`, but these binaries still construct bare `App`s through the duplicated fixture layer the migration exists to delete, so `rename_common` carries both layers side by side.
- **Instead**: migrate the six binaries and `navhistory_common` onto `Session`, then delete the App layer and re-evaluate whether `set_doc_db_for_test` is orphaned. Known driver gaps that blocked full migration, to close in `rune-fuzz` first: no out-of-order db-op delivery through checked steps (`deliver_db` is oldest-first; `merge_common::deliver_op_unchecked` is the workaround); the redivergence tracker only learns of external writes via `Action::DivergeDisk`; `Effects::raw`/timer arming invisible through `Session`; `ReadDir`/`ReadFile` Cmds dropped by the driver; a single rename-Cmd slot; no targeted `ClipboardRead` action; `SAVE-INFLIGHT-SM` rejects the legitimate `bind_new_now` Enter-materialize flip.
- **Done when**: no test binary constructs an `App` through a `*_common` fixture that duplicates `Session`, and the driver gaps above are either closed or individually recorded as deliberate.

### Sentinel-value residue
- **Where**: `crates/rune-tui/src/app.rs` (`frame_height/width: u16`, 0 = no resize yet — its own entry below defers it); `crates/rune-tui/src/filesearch/rank.rs` and `crates/rune-tui/src/messages/mod.rs` (`unwrap_or(usize::MAX)` — both documented deliberate orderings). (`rune-nav`'s empty-`PathBuf` root is fixed: `resolve` takes `Option<&Path>`.)
- **Wrong**: the class is otherwise closed (`CellMap`, table `buf`, `Cell.buf_offset` are `Option<u32>`; `App.root` is `Option<PathBuf>`); these are the remaining sites where an absent value borrows a valid-looking encoding.
- **Instead**: replace the two `usize::MAX` orderings with explicit `Option`-aware comparisons in the typed-newtypes pass.
- **Done when**: no site encodes "absent" as `usize::MAX`.

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

### Command palette: remaining imperative refusals not yet on the registry predicate
- **Where**: `crates/rune-tui/src/pane.rs`'s `handle_global_command` (`Trash`), `crates/rune-tui/src/rename.rs` (rename-in-flight), `crates/rune-tui/src/save.rs`/`workspace/close.rs` (save-in-flight, close-while-saving) — every one still refuses through its own inline `messages::warn`/`messages::info` call rather than a `registry::avail` predicate the palette can grey a row against.
- **Wrong**: a palette row for `trash`, `rename`, or `close` shows `Available` right up until Enter, then refuses with wording the palette never previewed — the same gap WP3 closed for Merge/ToggleReadOnly/TogglePin/Reload/read-only edits, just not closed everywhere yet.
- **Instead**: migrate arm-by-arm, one `registry/avail.rs` predicate per imperative refusal, following the WP3 pattern (predicate consulted at the top of the arm, same reason string old and new).
- **Done when**: no `GlobalCommand`/`keymap::Command` arm posts a refusal that isn't first visible as `Availability::Unavailable` on its registry row.

### Command palette: no in-palette mouse support
- **Where**: `crates/rune-tui/src/commands/mouse.rs`, `crates/rune-tui/src/palette/` — mouse routing only closes the palette on a click outside `geo.palette`; a wheel or drag while the palette is open falls through to the editor pane underneath instead of scrolling or selecting the palette's own row list.
- **Wrong**: the palette is otherwise fully keyboard-driven, but a mouse user scrolling or dragging over the open overlay edits the hidden document instead.
- **Instead**: route wheel/drag events landing inside `geo.palette` to the palette's row navigation instead of the editor.
- **Done when**: a wheel or drag inside the open palette moves its own selection/window rather than the editor underneath.

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
