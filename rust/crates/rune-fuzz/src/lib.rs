//! rune-fuzz: the headless session fuzzer for the Rust port. Drives the
//! real `rune_tui::app::update` against an in-memory `Vfs` with no
//! terminal, no clock, and no subprocess, checking named invariants after
//! every settled message. Mirrors the Go tree's `internal/fuzz` split:
//! action model -> driver -> snapshot/step-context -> pure invariant
//! checkers.
//!
//! # Invariant roster (21 total)
//!
//! Mirrors the convention at `internal/fuzz/session/session.go:1-11`: id,
//! one-line meaning, Go provenance where the plan names one. See
//! `invariant/mod.rs` for the checker-shape taxonomy (L0/L1/L2) and the
//! per-domain file split.
//!
//! - `NO-PANIC` — the driver caught an unwind (a `debug_assert!` tripping,
//!   or any other panic) while settling a message. Constructed directly by
//!   `driver.rs`, not a checker function.
//! - `CUR-BOUNDS` — every cursor's `position`/`anchor` is a valid,
//!   in-bounds, char-boundary byte offset (§1.3, §1.5).
//! - `CUR-ORDER` — cursors are ordered and non-overlapping (Go `C1`,
//!   `internal/fuzz/ui/textedit/textedit.go:254-267`).
//! - `CUR-ID` — at least one cursor; every id is non-zero and unique (Go
//!   `C2`, `textedit.go:269-287`).
//! - `BUF-LINE-INDEX` — the line index (`line_count`/`line_starts`/
//!   `line_ends`) is internally consistent with the buffer content (Go
//!   `B1`).
//! - `VERSION-MONOTONE` — `Buffer::version()`/`saved_version` never regress
//!   across a step (Go `B2`).
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
//! - `WRAP-RT` — `wrap_to_syntax(syntax_to_wrap(p)) == p` for every syntax
//!   point `p` in the in-domain rectangle (Go `WRAP-RT`; forward
//!   composition only, per G7; sampled per G19).
//! - `REDO-CLEAR` — a step that both bumps the version and pushes a new
//!   journal step always leaves `journal_pos == journal_len` (Go
//!   `REDO-CLEAR`, `internal/fuzz/driver/driver_verbatim.go`).
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
pub mod action;
pub mod driver;
pub mod generate;
pub mod invariant;
pub mod report;
pub mod script;
pub mod snapshot;
pub mod step;
