//! The reader thread: owns a single `SQLITE_OPEN_READ_ONLY` connection and
//! serves stale-tolerant display/immutable reads only — never a
//! decision-input (plan decision 8: "Every state-changing op re-derives its
//! inputs ... inside its own `BEGIN IMMEDIATE` tx on the writer ... The
//! reader handle's type exposes no decision-input methods"). Enforced here
//! by construction: [`ReaderRequestKind`] is the *only* surface this thread
//! exposes, and WP3+ may only ever add display-shaped variants to it — a
//! `saved_obs`/`current_seq`/"newest observation for a decision" read must
//! never be added to this enum, no matter how convenient.
//!
//! WP2 ships only [`ReaderRequestKind::Ping`], a placeholder proving the
//! thread and its read-only connection are live end-to-end; real reads
//! (blob content, sync badges, ...) land in WP3+.

use std::panic::{self, AssertUnwindSafe};
use std::sync::mpsc::{self, Sender};
use std::thread;

use rusqlite::{Connection, OpenFlags};

use crate::Error;

/// A display/immutable read the reader thread can serve. See the module doc
/// for why this enum's membership is a hard boundary, not a convenience.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReaderRequestKind {
    /// Round-trips `SELECT 1` — proves the reader thread and its
    /// `SQLITE_OPEN_READ_ONLY` connection are alive and can run a query.
    Ping,
    /// Decompresses and hash-verifies the blob stored under `hash` (plan
    /// Hard rules: "reader.rs may gain get_blob/display reads only") —
    /// stale-tolerant content, never a decision input (Decision 8).
    GetBlob { hash: String },
}

/// The reply to a [`ReaderRequestKind`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReaderReply {
    Pong,
    /// The raw bytes stored under the requested hash — never decoded as
    /// text here (blob.rs's module doc: content is only ever validated as
    /// UTF-8 at the point it re-enters a `String`-typed buffer, which is
    /// not this stale-tolerant display path's concern).
    Blob(Vec<u8>),
}

struct Request {
    kind: ReaderRequestKind,
    reply: Sender<Result<ReaderReply, Error>>,
}

/// A live handle to the reader thread.
pub struct ReaderHandle {
    sender: Sender<Request>,
    thread: Option<thread::JoinHandle<()>>,
}

impl ReaderHandle {
    /// Sends `kind` to the reader thread and blocks for its reply. Intended
    /// to be called from a spawned `Cmd` (off the main `update` thread),
    /// never from `update` itself.
    pub fn query(&self, kind: ReaderRequestKind) -> Result<ReaderReply, Error> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.sender
            .send(Request {
                kind,
                reply: reply_tx,
            })
            .map_err(|_| Error::ReaderGone)?;
        reply_rx.recv().map_err(|_| Error::ReaderGone)?
    }

    /// Drops the request side and blocks until the reader thread observes
    /// disconnection and exits — deterministic, no polling loop.
    pub fn shutdown(self) {
        let ReaderHandle { sender, thread } = self;
        drop(sender);
        if let Some(thread) = thread {
            let _ = thread.join();
        }
    }
}

/// Opens a fresh `SQLITE_OPEN_READ_ONLY` connection to `path` and spawns the
/// reader thread. `path` must already have its schema applied (the writer
/// connection does this first, per `store::open`'s ordering) — a read-only
/// connection cannot create tables.
pub fn spawn(path: &str) -> Result<ReaderHandle, Error> {
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY
        | OpenFlags::SQLITE_OPEN_NO_MUTEX
        | OpenFlags::SQLITE_OPEN_URI;
    let conn = Connection::open_with_flags(path, flags)?;
    crate::store::apply_connection_pragmas(&conn)?;

    let (sender, receiver) = mpsc::channel::<Request>();
    let thread = thread::spawn(move || reader_loop(conn, receiver));
    Ok(ReaderHandle {
        sender,
        thread: Some(thread),
    })
}

fn reader_loop(conn: Connection, receiver: mpsc::Receiver<Request>) {
    while let Ok(req) = receiver.recv() {
        // Unlike the writer thread, a panic here doesn't leave a
        // transaction in an ambiguous state (reads never mutate) — catch
        // it (repo-wide rule: "any long-lived thread must catch panics
        // like spawn_cmd does"), reply with an error, and keep serving
        // subsequent requests rather than parking the whole thread.
        let outcome = panic::catch_unwind(AssertUnwindSafe(|| execute(&conn, req.kind)))
            .unwrap_or_else(|_| Err(Error::ReaderGone));
        // A send failure means the caller stopped waiting (dropped its
        // receiver) — there is no one left to hand `outcome` to.
        let _ = req.reply.send(outcome);
    }
}

fn execute(conn: &Connection, kind: ReaderRequestKind) -> Result<ReaderReply, Error> {
    match kind {
        ReaderRequestKind::Ping => {
            conn.query_row("SELECT 1", [], |_row| Ok(()))?;
            Ok(ReaderReply::Pong)
        }
        ReaderRequestKind::GetBlob { hash } => {
            crate::blob::get_blob(conn, &hash).map(ReaderReply::Blob)
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use super::*;

    #[test]
    fn ping_round_trips_pong() {
        let uri = "file:rune-db-reader-test-ping?mode=memory&cache=shared";
        // The reader can't create the shared in-memory database on its own
        // (SQLITE_OPEN_READ_ONLY, no CREATE flag) — open a throwaway
        // read-write connection first to bring it into existence and apply
        // schema, exactly as `store::open` sequences writer-before-reader.
        let bootstrap = Connection::open_with_flags(
            uri,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_URI,
        )
        .expect("bootstrap shared memdb");
        crate::schema::apply(&bootstrap).expect("apply schema");

        let handle = spawn(uri).expect("spawn reader");
        let reply = handle.query(ReaderRequestKind::Ping).expect("ping");
        assert_eq!(reply, ReaderReply::Pong);
        handle.shutdown();

        drop(bootstrap);
    }

    #[test]
    fn get_blob_round_trips_through_the_reader() {
        let uri = "file:rune-db-reader-test-getblob?mode=memory&cache=shared";
        let bootstrap = Connection::open_with_flags(
            uri,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_URI,
        )
        .expect("bootstrap shared memdb");
        crate::schema::apply(&bootstrap).expect("apply schema");
        let hash = crate::blob::put_blob(&bootstrap, b"reader blob content").expect("put blob");

        let handle = spawn(uri).expect("spawn reader");
        let reply = handle
            .query(ReaderRequestKind::GetBlob { hash })
            .expect("get blob");
        assert_eq!(reply, ReaderReply::Blob(b"reader blob content".to_vec()));
        handle.shutdown();

        drop(bootstrap);
    }
}
