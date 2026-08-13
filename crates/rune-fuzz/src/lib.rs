//! rune-fuzz: the headless session fuzzer for `rune`. Drives the
//! real `rune_tui::app::update` against an in-memory `Vfs` with no
//! terminal, no clock, and no subprocess, checking named invariants after
//! every settled message: action model -> driver -> snapshot/step-context
//! -> pure invariant checkers.
//!
//! # Invariant roster (39 total)
//!
//! Each entry: id, one-line meaning. See `invariant/mod.rs` for the
//! checker-shape taxonomy (L0/L1/L2) and the per-domain file split. This
//! crate deliberately does NOT depend on `rune-db` (plan WP7.S10) — no
//! journal-coalescing/recovery-store invariant (a durable snapshot
//! cadence, a materialize ack sequence, …) can be expressed here; that
//! layer's own crate carries its own tests.
//!
//! - `NO-PANIC` — the driver caught an unwind (a `debug_assert!` tripping,
//!   or any other panic) anywhere in a session: settling a message, running
//!   the display-pipeline checkers, or setting the session up. Constructed
//!   by `guard`, which also names the file, line, and backtrace it came
//!   from — not a checker function.
//! - `SAVE-SINGLE-FLIGHT` — a second in-flight save `Cmd` never arrives
//!   while one is already pending (G9: at most one save `Cmd` is ever
//!   outstanding). Also constructed directly by `driver.rs`.
//! - `CUR-BOUNDS` — every cursor's `position`/`anchor` is a valid,
//!   in-bounds, char-boundary byte offset.
//! - `CUR-ORDER` — cursors are ordered and non-overlapping.
//! - `CUR-ID` — at least one cursor; every id is non-zero and unique.
//! - `CUR-NO-CARET-HIDDEN` — when `caret_visible` is false, no rendered
//!   cell carries `Modifier::REVERSED` (sampled per G19).
//! - `BUF-LINE-INDEX` — the line index (`line_count`/`line_starts`/
//!   `line_ends`) is internally consistent with the buffer content.
//! - `VERSION-MONOTONE` — `Buffer::version()`/`saved_version` never regress
//!   across a step.
//! - `NAV-BOUNDS` — every recorded nav place is an in-bounds, char-boundary
//!   byte offset into its document, and `nav_current` stays within the
//!   places list.
//! - `PANE-NO-BLEED` — a keystroke aimed at chrome (no modal up, focus off
//!   `Pane::Editor`, active document unchanged) never mutates the document
//!   behind it — the rule the `UNDO-TOTAL`/`REDO-TOTAL` harness fix
//!   (`driver.rs::restore_editor_focus`) rests on.
//! - `LAYOUT-FITS` — every rect `layout::geometry` hands `render::draw`
//!   stays inside its frame, and the left-column panes it carves out never
//!   overlap each other or spill past the block that borders them.
//! - `LAYOUT-TILES` — no frame column inside `main` goes unpainted by both
//!   `left_block` and `center`.
//! - `SYNC-IDEMPOTENT` — a second `app.sync_view()` with no intervening
//!   message reproduces the same rendered rows and the same
//!   `viewport.scroll_row` (sampled per G19).
//! - `CELL-OFFSET` — every rendered `Cell.buf_offset` is `-1` or a valid,
//!   in-bounds, char-boundary byte offset, and implies `width >= 1`
//!   (sampled per G19).
//! - `CELL-NO-EOL` — no rendered cell carries `\n`/`\r` (sampled per G19).
//! - `CELL-ORDER` — within a row, non-negative `buf_offset`s never go
//!   backwards (sampled per G19).
//! - `TABLE-ROW-WIDTH` — within one contiguous table, every row (content
//!   or a synthesised border) has the same summed cell width (plan WP5.S3;
//!   sampled per G19).
//! - `TABLE-SYNTHETIC-DECORATIVE` — every cell of a synthesised border row
//!   carries `buf_offset == -1` (plan WP5.S4; sampled per G19).
//! - `WRAP-RT` — `wrap_to_syntax(syntax_to_wrap(p)) == p` for every syntax
//!   point `p` in the in-domain rectangle (forward composition only, per
//!   G7; sampled per G19).
//! - `REDO-CLEAR` — a step that both bumps the version and pushes a new
//!   journal step always leaves `journal_pos == journal_len`.
//! - `SAVE-INFLIGHT-SM` — `save_in_flight` flips false->true only on a
//!   `Command::Save` key, and true->false only on `SaveDone` (G9).
//! - `QUIT-CHORD` — `should_quit` flips false->true only on the SAME quit
//!   chord already armed (protocol only, NOT a dirty check — G15);
//!   `Msg::Quit` is out of this checker's domain entirely — this headless
//!   driver never constructs one (`step::MsgTag`'s own docs).
//! - `CONFIRM-GEN` — a `ConfirmTimeout` clears `pending_quit` iff its
//!   generation matches the armed one.
//! - `GUARD-ANSWERED` — a key that actually answers a `DirtyQuit` Guard
//!   always leaves the app quitting, mid-save, or showing an explanatory
//!   status — never back in the bit-for-bit identical Guard.
//! - `PASTE-VERBATIM` — a paste into a collapsed cursor inserts exactly the
//!   pasted bytes at the caret, unfiltered (the only path that can carry
//!   control bytes, G3).
//! - `SAVE-VERBATIM` — a successful save's on-disk bytes byte-equal the
//!   bytes it was constructed with.
//! - `SAVE-CLEAN-MATCHES-DISK` — once clean with a delivered save and none
//!   pending, disk bytes byte-equal the current content.
//! - `CLIP-OSC52` — a `Copy`/`Cut` over a non-empty selection emits an OSC
//!   52 raw chunk whose decoded payload byte-equals the selected text.
//! - `UNDO-TOTAL` — pressing undo down to `journal_pos == 0` restores the
//!   seed content byte-for-byte; content-only, since undo does not restore
//!   `version` (G5; end-of-session, once).
//! - `REDO-TOTAL` — pressing redo back up to `journal_pos == journal_len`
//!   restores the pre-undo-drive content byte-for-byte (end-of-session,
//!   once, immediately after `UNDO-TOTAL`).
//! - `HL-CLAMPED` — every stored highlight span satisfies `start < end`,
//!   `end <= content.len()`, and both endpoints are `char` boundaries
//!   (plan WP7.S7).
//! - `HL-STALE-DROP` — a `Msg::Highlighted` reply whose delivered version no
//!   longer matches the live buffer leaves the stored spans unchanged
//!   (plan WP7.S7, `[R2]`).
//! - `HL-NO-REFLOW` — a `Msg::Highlighted` step never changes `content`,
//!   `version`, the journal, `is_dirty`, or any rendered cell's
//!   `buf_offset`/`width` — it is a pure style overlay (plan WP7.S7,
//!   decision 1).
//! - `MERGE-DOC-ACTIVE` — whenever merge mode is `Active`, the document it
//!   names is still open and is the active document.
//! - `MERGE-SAVE-BLOCKED` — a `Command::Save` key pressed while merge is
//!   `Active` with unresolved blocks never arms a save.
//! - `MERGE-KEY-FEEDBACK` — every key dispatched while merge is `Active`
//!   and the Editor pane is focused leaves an observable trace (buffer,
//!   cursors, scroll, merge state, or status), never a silent swallow.
//! - `MERGE-TITLE-CLEARED` — once merge mode is fully `Inactive`, no open
//!   document's `display_name` still reads the merge retitle.
//! - `MERGE-NO-INSTANT-REDIVERGENCE` — once a retired merge leaves its
//!   document reconciled, no later step re-classifies it `Diverged`
//!   without something genuinely moving underneath it again. Stateful
//!   across steps, driven directly by `driver.rs`, like `SAVE-SINGLE-
//!   FLIGHT`.
//! - `SAVE-AGREES-WITH-DIVERGENCE` — a publish never commits once the
//!   store's own prepare-time verdict said the disk holds changes the
//!   buffer does not, unless the user explicitly forced it.
pub mod action;
pub mod driver;
pub use driver::Session;
pub mod fault;
pub mod generate;
pub mod guard;
mod hash;
pub mod invariant;
pub mod report;
pub mod script;
pub mod snapshot;
pub mod step;
#[cfg(test)]
mod test_support;
pub mod wal;
