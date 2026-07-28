//! rune-fuzz: the headless session fuzzer for the Rust port. Drives the
//! real `rune_tui::app::update` against an in-memory `Vfs` with no
//! terminal, no clock, and no subprocess, checking named invariants after
//! every settled message. Mirrors the Go session fuzzer's split:
//! action model -> driver -> snapshot/step-context -> pure invariant
//! checkers.
//!
//! # Invariant roster (27 total)
//!
//! Mirrors the Go fuzzer's roster convention: id,
//! one-line meaning, Go provenance where the plan names one. See
//! `invariant/mod.rs` for the checker-shape taxonomy (L0/L1/L2) and the
//! per-domain file split. This crate deliberately does NOT depend on
//! `rune-db` (plan WP7.S10) — no journal-coalescing/recovery-store
//! invariant (a durable snapshot cadence, a materialize ack sequence, …)
//! can be expressed here; that layer's own crate carries its own tests.
//!
//! - `NO-PANIC` — the driver caught an unwind (a `debug_assert!` tripping,
//!   or any other panic) while settling a message. Constructed directly by
//!   `driver.rs`, not a checker function.
//! - `CUR-BOUNDS` — every cursor's `position`/`anchor` is a valid,
//!   in-bounds, char-boundary byte offset (§1.3, §1.5).
//! - `CUR-ORDER` — cursors are ordered and non-overlapping (Go `C1`).
//! - `CUR-ID` — at least one cursor; every id is non-zero and unique (Go
//!   `C2`).
//! - `BUF-LINE-INDEX` — the line index (`line_count`/`line_starts`/
//!   `line_ends`) is internally consistent with the buffer content (Go
//!   `B1`).
//! - `VERSION-MONOTONE` — `Buffer::version()`/`saved_version` never regress
//!   across a step (Go `B2`).
//! - `PANE-NO-BLEED` — a keystroke aimed at chrome (no modal up, focus off
//!   `Pane::Editor`, active document unchanged) never mutates the document
//!   behind it — the rule the `UNDO-TOTAL`/`REDO-TOTAL` harness fix
//!   (`driver.rs::restore_editor_focus`) rests on.
//! - `SYNC-IDEMPOTENT` — a second `app.sync_view()` with no intervening
//!   message reproduces the same rendered rows and the same
//!   `viewport.scroll_row` (§8 "Render Purity"; sampled per G19).
//! - `CELL-OFFSET` — every rendered `Cell.buf_offset` is `-1` or a valid,
//!   in-bounds, char-boundary byte offset, and implies `width >= 1` (Go
//!   `R4`/`R5`; sampled per G19).
//! - `CELL-NO-EOL` — no rendered cell carries `\n`/`\r` (Go `R8`; sampled
//!   per G19).
//! - `CELL-ORDER` — within a row, non-negative `buf_offset`s never go
//!   backwards (Go `R3`; sampled per G19).
//! - `TABLE-ROW-WIDTH` — within one contiguous table, every row (content
//!   or a synthesised border) has the same summed cell width (plan WP5.S3;
//!   sampled per G19).
//! - `TABLE-SYNTHETIC-DECORATIVE` — every cell of a synthesised border row
//!   carries `buf_offset == -1` (plan WP5.S4; sampled per G19).
//! - `WRAP-RT` — `wrap_to_syntax(syntax_to_wrap(p)) == p` for every syntax
//!   point `p` in the in-domain rectangle (Go `WRAP-RT`; forward
//!   composition only, per G7; sampled per G19).
//! - `REDO-CLEAR` — a step that both bumps the version and pushes a new
//!   journal step always leaves `journal_pos == journal_len` (Go
//!   `REDO-CLEAR`).
//! - `SAVE-INFLIGHT-SM` — `save_in_flight` flips false->true only on a
//!   `Command::Save` key, and true->false only on `SaveDone` (G9).
//! - `QUIT-CHORD` — `should_quit` flips false->true only on `Msg::Quit` or
//!   the SAME quit chord already armed (protocol only, NOT a dirty check —
//!   G15).
//! - `CONFIRM-GEN` — a `ConfirmTimeout` clears `pending_quit` iff its
//!   generation matches the armed one.
//! - `PASTE-VERBATIM` — a paste into a collapsed cursor inserts exactly the
//!   pasted bytes at the caret, unfiltered (§1.4.5; the only path that can
//!   carry control bytes, G3).
//! - `SAVE-VERBATIM` — a successful save's on-disk bytes byte-equal the
//!   bytes it was constructed with (§1.4.5; Go `SAVE-VERBATIM`,
//!   `driver_verbatim.go:78-109`).
//! - `SAVE-CLEAN-MATCHES-DISK` — once clean with a delivered save and none
//!   pending, disk bytes byte-equal the current content (§1.4.8).
//! - `CLIP-OSC52` — a `Copy`/`Cut` over a non-empty selection emits an OSC
//!   52 raw chunk whose decoded payload byte-equals the selected text
//!   (§1.4.5).
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
pub mod action;
pub mod driver;
pub mod generate;
pub mod invariant;
pub mod report;
pub mod script;
pub mod snapshot;
pub mod step;
