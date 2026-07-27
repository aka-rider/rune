//! `rune-db`'s error type. No `anyhow`/`thiserror` — matches the existing
//! workspace convention (`rune-tui::runtime::Msg::SaveDone` already carries
//! `Result<(), String>` for exactly this reason). This crate keeps a real
//! enum internally so `retry.rs` can inspect the underlying
//! `rusqlite::Error`'s extended result code; callers outside the crate that
//! only want a message flatten it with `.to_string()` at the `DbEvent`/`Msg`
//! boundary (WP5).

use std::fmt;

/// Everything that can go wrong opening or operating a `rune-db` `Store`.
#[derive(Debug)]
pub enum Error {
    /// A SQLite operation failed. Wraps the underlying `rusqlite::Error`
    /// verbatim so `retry::classify` can inspect
    /// `sqlite_extended_error_code()`.
    Sqlite(rusqlite::Error),
    /// A filesystem operation in the open ladder (`mkdir_all`, stat) failed.
    /// Never surfaced on its own for the degraded fallback path — see
    /// `store::open`'s doc comment — only for the near-degenerate case where
    /// even the `:memory:` fallback can't be opened.
    Io(std::io::Error),
    /// The writer thread's bounded queue (`sync_channel(1024)`, plan
    /// Assumption A2) is full — `try_send`'s immediate-error contract
    /// (Gotchas: "the enqueue path must use `try_send`... never block
    /// `update`").
    WriterQueueFull,
    /// The writer thread has already exited (parked after a caught panic,
    /// or the `Store` was dropped) and can no longer accept work.
    WriterGone,
    /// The reader thread has already exited and can no longer serve reads.
    ReaderGone,
    /// This process's own `sessions` row could not be established. Hard
    /// failure (`store.go`'s `openStoreAt` doc comment: session identity is
    /// load-bearing for every subsequent write) — there is no fallback left
    /// once even the `:memory:` open ladder rung has been reached.
    SessionEstablish(String),
    /// `PRAGMA journal_mode=WAL` did not report back `"wal"` on a
    /// file-backed connection (plan Gotchas: "verify the returned string is
    /// `wal`") — treated as an open-ladder failure so the caller falls
    /// through to the next rung rather than silently running without WAL's
    /// multi-connection concurrency guarantees.
    WalModeUnavailable(String),
    /// A `events`/`snapshots` JSON payload (edits or cursors) failed to
    /// parse. Port of `journal.go`'s §1.3 discipline: `UndoPeek`/`RedoPeek`
    /// surface a corrupt payload as an error, NEVER silently fold it into
    /// `ok=false` ("nothing to undo/redo") — a corrupt row is a Tolerable
    /// halt with the buffer kept, not an empty journal
    /// (`journal.go:203-206`).
    CorruptPayload(String),
    /// `get_blob` decompressed a row whose SHA-256 does not match its own
    /// `hash` key — blob rot / bit-flip detection (port of
    /// `snapshot.go:67-70`). Surfaced, never silently returned as if it were
    /// the original content.
    BlobHashMismatch { hash: String, got: String },
    /// A replay (`snapshot::recover_document`) attempted to apply an
    /// `AppliedEdit` batch that does not fit the buffer it was replayed
    /// against — an out-of-range or malformed journal row. Wraps
    /// `rune_core::buffer::BufferError` (port of the failure mode
    /// `buffer.ReplayForward` used to silently clamp/skip; §1.3 forbids
    /// that here — see `snapshot.rs`'s module doc).
    ReplayFailed(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Sqlite(e) => write!(f, "sqlite: {e}"),
            Error::Io(e) => write!(f, "io: {e}"),
            Error::WriterQueueFull => write!(f, "writer queue full"),
            Error::WriterGone => write!(f, "writer thread is gone"),
            Error::ReaderGone => write!(f, "reader thread is gone"),
            Error::SessionEstablish(msg) => write!(f, "establish session: {msg}"),
            Error::WalModeUnavailable(got) => {
                write!(f, "PRAGMA journal_mode=WAL returned {got:?}, not \"wal\"")
            }
            Error::CorruptPayload(msg) => write!(f, "corrupt journal payload: {msg}"),
            Error::BlobHashMismatch { hash, got } => write!(
                f,
                "get blob {hash}: content hash mismatch (corrupt blob): got {got}"
            ),
            Error::ReplayFailed(msg) => write!(f, "replay failed: {msg}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Sqlite(e) => Some(e),
            Error::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Error::Sqlite(e)
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}
