//! The one chokepoint for this crate's best-effort background diagnostics —
//! GC/checkpoint/optimize outcomes that are logged, never surfaced, because
//! the caller they'd surface to has already moved on (a quiet-period sweep,
//! a shutdown housekeeping pass). Nothing here is a durability fact: every
//! path that IS one (a journal write, a materialize outcome) goes through
//! `Error`/`DbEvent`, never this function. Centralizing the write site
//! means a future structured-log/telemetry swap touches one place instead
//! of grepping the crate for `eprintln!`.

/// Logs a best-effort diagnostic for a background operation whose failure
/// is deliberately swallowed by its caller (housekeeping, not user-data
/// state). `msg` should already read as a complete, self-contained line.
pub(crate) fn background_note(msg: &str) {
    eprintln!("rune-db: {msg}");
}
