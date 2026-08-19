//! `rune-db`'s error type. No `anyhow`/`thiserror` — a plain hand-written
//! enum whose variants each keep their source's real type (a `rusqlite::
//! Error`, a `serde_json::Error`, a `rune_core::buffer::BufferError`, ...)
//! rather than flattening to a message on construction, so `retry.rs` can
//! inspect the underlying SQLite extended result code and any caller can
//! match on what actually failed. A caller that only wants a message still
//! gets one, unchanged, from `Display`.

use std::fmt;

use crate::ids::{DocId, SessionId};

/// Why [`Error::SessionEstablish`] failed — either rung of `establish_
/// session`'s two fallible steps, kept distinct so a caller can tell "could
/// not even read our own pid's start time" from "the INSERT itself failed".
#[derive(Debug)]
pub enum SessionEstablishReason {
    /// `proc_started_at` returned nothing for this process's own pid —
    /// there is no OS-reported start time to compare against on future
    /// liveness checks.
    NoStartTime { pid: i64 },
    /// The `sessions` row INSERT itself failed.
    Sqlite(rusqlite::Error),
}

impl fmt::Display for SessionEstablishReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SessionEstablishReason::NoStartTime { pid } => {
                write!(f, "could not read start time of own pid {pid}")
            }
            SessionEstablishReason::Sqlite(e) => write!(f, "{e}"),
        }
    }
}

/// Why [`Error::CorruptPayload`] failed — the three ways a journal/snapshot
/// row can be corrupt, kept distinct so a caller can inspect the underlying
/// decode error rather than only ever seeing its rendered message.
#[derive(Debug)]
pub enum CorruptPayloadReason {
    /// A snapshot blob's bytes are not valid UTF-8 — this crate's
    /// `Buffer`/`AppliedEdit` model is `String`-based throughout (see
    /// `snapshot.rs`'s module doc).
    NonUtf8Blob {
        hash: String,
        doc_id: DocId,
        source: std::string::FromUtf8Error,
    },
    /// An `events`/`snapshots` JSON payload (edits or cursors) failed to
    /// serialize or parse.
    Json(serde_json::Error),
    /// A `cursors` payload named cursor id 0 — `CursorId` is non-zero by
    /// construction, so a zero id can only have come from a corrupt row.
    #[cfg(feature = "test-support")]
    InvalidCursorId { id: u32 },
}

impl fmt::Display for CorruptPayloadReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CorruptPayloadReason::NonUtf8Blob {
                hash,
                doc_id,
                source,
            } => write!(
                f,
                "snapshot blob {hash} for doc {doc_id}: non-utf8 content: {source}"
            ),
            CorruptPayloadReason::Json(e) => write!(f, "{e}"),
            #[cfg(feature = "test-support")]
            CorruptPayloadReason::InvalidCursorId { id } => {
                write!(f, "cursor id {id} must be non-zero")
            }
        }
    }
}

/// A journal row that [`crate::snapshot::recover_document`] could not
/// replay onto the buffer it was replaying against — an out-of-range or
/// malformed `AppliedEdit` batch. Wraps `rune_core::buffer::BufferError`,
/// surfacing the failure as corruption rather than silently clamping or
/// skipping it (see `snapshot.rs`'s module doc).
#[derive(Debug)]
pub struct ReplayFailure {
    pub doc_id: DocId,
    pub session_id: SessionId,
    pub seq: crate::ids::Seq,
    pub source: rune_core::buffer::BufferError,
}

impl fmt::Display for ReplayFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ReplayFailure {
            doc_id,
            session_id,
            seq,
            source,
        } = self;
        write!(
            f,
            "doc {doc_id} session {session_id} at seq {seq}: {source}"
        )
    }
}

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
    /// failure (session identity is load-bearing for every subsequent
    /// write) — there is no fallback left once even the `:memory:` open
    /// ladder rung has been reached.
    SessionEstablish(SessionEstablishReason),
    /// `PRAGMA journal_mode=WAL` did not report back `"wal"` on a
    /// file-backed connection (plan Gotchas: "verify the returned string is
    /// `wal`") — treated as an open-ladder failure so the caller falls
    /// through to the next rung rather than silently running without WAL's
    /// multi-connection concurrency guarantees.
    WalModeUnavailable(String),
    /// A `events`/`snapshots` JSON payload (edits or cursors) failed to
    /// parse. `undo_peek`/`redo_peek` surface a corrupt payload as an
    /// error, NEVER silently fold it into `ok=false` ("nothing to
    /// undo/redo") — a corrupt row is a halt with the buffer kept, not an
    /// empty journal.
    CorruptPayload(CorruptPayloadReason),
    /// `get_blob` decompressed a row whose SHA-256 does not match its own
    /// `hash` key — blob rot / bit-flip detection. Surfaced, never
    /// silently returned as if it were the original content.
    BlobHashMismatch { hash: String, got: String },
    /// A replay (`snapshot::recover_document`) attempted to apply an
    /// `AppliedEdit` batch that does not fit the buffer it was replayed
    /// against — an out-of-range or malformed journal row.
    ReplayFailed(Box<ReplayFailure>),
    /// A lookup that returns a genuine error rather than a silent zero
    /// value — e.g. `get_observation` on an id with no row. Never used for
    /// a legitimate "not found" outcome a caller treats as ordinary
    /// control flow (those stay `Option`/`bool`, per this crate's own
    /// Options-for-absent-facts rule) — only for a caller-supplied
    /// reference (an `ObsId`, a bound path) that MUST resolve.
    NotFound(String),
    /// A WP4 business-rule refusal with no dedicated variant (e.g.
    /// `Materialize`'s "no path bound (untitled document)",
    /// `ResolveAbandon`'s "not a resolve adoption" refusal), or `load.rs`'s
    /// own non-UTF-8 disk read (this crate's `Buffer`/`AppliedEdit` model
    /// is `String`-based throughout and cannot tolerate arbitrary bytes —
    /// see `load.rs`'s doc comment). `probe.rs`/`materialize.rs` no longer
    /// hit this case: their disk-sourced reads never need to decode as text
    /// at all (`blob.rs`'s module doc) — only `load.rs`'s return path,
    /// which must produce a genuine `String` for the edit buffer, still can.
    Invalid(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Sqlite(e) => write!(f, "sqlite: {e}"),
            Error::Io(e) => write!(f, "io: {e}"),
            Error::WriterQueueFull => write!(f, "writer queue full"),
            Error::WriterGone => write!(f, "writer thread is gone"),
            Error::ReaderGone => write!(f, "reader thread is gone"),
            Error::SessionEstablish(reason) => write!(f, "establish session: {reason}"),
            Error::WalModeUnavailable(got) => {
                write!(f, "PRAGMA journal_mode=WAL returned {got:?}, not \"wal\"")
            }
            Error::CorruptPayload(reason) => write!(f, "corrupt journal payload: {reason}"),
            Error::BlobHashMismatch { hash, got } => write!(
                f,
                "get blob {hash}: content hash mismatch (corrupt blob): got {got}"
            ),
            Error::ReplayFailed(failure) => write!(f, "replay failed: {failure}"),
            Error::NotFound(msg) => write!(f, "not found: {msg}"),
            Error::Invalid(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Sqlite(e) => Some(e),
            Error::Io(e) => Some(e),
            Error::SessionEstablish(SessionEstablishReason::Sqlite(e)) => Some(e),
            Error::CorruptPayload(CorruptPayloadReason::NonUtf8Blob { source, .. }) => Some(source),
            Error::CorruptPayload(CorruptPayloadReason::Json(e)) => Some(e),
            Error::ReplayFailed(failure) => Some(&failure.source),
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
