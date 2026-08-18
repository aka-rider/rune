//! The reader thread: owns a single `SQLITE_OPEN_READ_ONLY` connection and
//! serves stale-tolerant display/immutable reads only — never a
//! decision-input. Every state-changing op re-derives its inputs inside its
//! own `BEGIN IMMEDIATE` tx on the writer; the reader handle's type exposes
//! no decision-input methods. Enforced here by construction:
//! [`ReaderRequestKind`] is the *only* surface this thread exposes, and
//! future additions may only ever be display-shaped variants of it — a
//! `saved_obs`/`current_seq`/"newest observation for a decision" read must
//! never be added to this enum, no matter how convenient.
//!
//! Currently only [`ReaderRequestKind::Ping`] ships, a placeholder proving
//! the thread and its read-only connection are live end-to-end; real reads
//! (blob content, sync badges, ...) land later.

use std::panic::{self, AssertUnwindSafe};
use std::sync::mpsc::{self, Sender};
use std::thread;

use rusqlite::Connection;

use crate::Error;

/// A display/immutable read the reader thread can serve. See the module doc
/// for why this enum's membership is a hard boundary, not a convenience.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReaderRequestKind {
    /// Round-trips `SELECT 1` — proves the reader thread and its
    /// `SQLITE_OPEN_READ_ONLY` connection are alive and can run a query.
    Ping,
    /// Decompresses and hash-verifies the blob stored under `hash` —
    /// stale-tolerant display content, never a decision input.
    GetBlob {
        hash: String,
    },
    /// The `limit` most recently used search queries, newest first — a
    /// history list for the search bar's UI, stale-tolerant and never
    /// consulted to make a decision, so it belongs on this thread exactly
    /// like `GetBlob`.
    RecentSearches {
        limit: u32,
    },
    /// The `limit` most recently opened real-file document paths, newest
    /// first — the fuzzy file finder's own MRU list, exactly as
    /// stale-tolerant and non-decision-input as `RecentSearches`.
    RecentDocuments {
        limit: u32,
    },
    RecentCommands {
        limit: u32,
    },
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
    /// `RecentSearches`'s reply: MRU-first query strings.
    RecentSearches(Vec<String>),
    /// `RecentDocuments`'s reply: MRU-first document paths.
    RecentDocuments(Vec<String>),
    RecentCommands(Vec<String>),
}

struct Request {
    kind: ReaderRequestKind,
    reply: Sender<Result<ReaderReply, Error>>,
}

/// What travels over the reader thread's channel. `Shutdown` is an explicit
/// sentinel: because [`ReaderQuery`] clones the sender, the channel may
/// never disconnect on its own, so termination cannot rely on sender-drop —
/// the loop must be told to stop.
enum Msg {
    Query(Request),
    Shutdown,
}

/// A live handle to the reader thread.
pub struct ReaderHandle {
    sender: Sender<Msg>,
    thread: Option<thread::JoinHandle<()>>,
}

impl ReaderHandle {
    /// Sends `kind` to the reader thread and blocks for its reply. Intended
    /// to be called from a spawned `Cmd` (off the main `update` thread),
    /// never from `update` itself.
    pub fn query(&self, kind: ReaderRequestKind) -> Result<ReaderReply, Error> {
        query_over(&self.sender, kind)
    }

    /// Sends the explicit `Shutdown` sentinel and blocks until the reader
    /// thread exits — deterministic, no polling loop. A send failure means
    /// the loop is already gone, which is exactly the state being asked
    /// for, so it is ignored. This must never wait on channel
    /// disconnection: [`ReaderQuery`] clones keep the channel connected
    /// indefinitely, and shutdown may not be hostage to their lifetimes.
    pub fn shutdown(self) {
        let ReaderHandle { sender, thread } = self;
        let _ = sender.send(Msg::Shutdown);
        drop(sender);
        if let Some(thread) = thread {
            let _ = thread.join();
        }
    }

    /// A cloneable query-only split of this handle — same `query` surface,
    /// no thread ownership. For a caller (`rune-tui`) that needs to move a
    /// reader reference into a `Box<dyn FnOnce() + Send>` `Cmd` closure,
    /// where `ReaderHandle` itself can't go (it isn't `Clone`, and moving it
    /// would hand the closure ownership/shutdown of the thread).
    pub fn as_query(&self) -> ReaderQuery {
        ReaderQuery(self.sender.clone())
    }
}

/// A cloneable handle to the reader thread's query surface only — see
/// [`ReaderHandle::as_query`]. Thread ownership/shutdown stays with
/// `ReaderHandle`; this is just the `Sender` side, cloned. A live
/// `ReaderQuery` can never keep the reader thread alive past
/// `ReaderHandle::shutdown`: the thread exits on an explicit sentinel, not
/// on channel disconnection, and a query sent after that returns
/// `Error::ReaderGone` instead of blocking.
#[derive(Clone)]
pub struct ReaderQuery(Sender<Msg>);

impl ReaderQuery {
    /// Identical contract to [`ReaderHandle::query`].
    pub fn query(&self, kind: ReaderRequestKind) -> Result<ReaderReply, Error> {
        query_over(&self.0, kind)
    }
}

fn query_over(sender: &Sender<Msg>, kind: ReaderRequestKind) -> Result<ReaderReply, Error> {
    let (reply_tx, reply_rx) = mpsc::channel();
    sender
        .send(Msg::Query(Request {
            kind,
            reply: reply_tx,
        }))
        .map_err(|_| Error::ReaderGone)?;
    reply_rx.recv().map_err(|_| Error::ReaderGone)?
}

/// Opens a fresh `SQLITE_OPEN_READ_ONLY` connection to `path` and spawns the
/// reader thread. `path` must already have its schema applied (the writer
/// connection does this first, per `store::open`'s ordering) — a read-only
/// connection cannot create tables.
pub fn spawn(path: &str) -> Result<ReaderHandle, Error> {
    let conn = crate::conn::open_read_replica(path)?;

    let (sender, receiver) = mpsc::channel::<Msg>();
    let thread = thread::spawn(move || reader_loop(&conn, &receiver));
    Ok(ReaderHandle {
        sender,
        thread: Some(thread),
    })
}

// Exits on the `Shutdown` sentinel; the `recv()`-disconnect exit stays as a
// fallback so a `ReaderHandle` dropped without calling `shutdown` (and no
// surviving `ReaderQuery` clones) still terminates the thread.
fn reader_loop(conn: &Connection, receiver: &mpsc::Receiver<Msg>) {
    while let Ok(msg) = receiver.recv() {
        let req = match msg {
            Msg::Query(req) => req,
            Msg::Shutdown => return,
        };
        // Unlike the writer thread, a panic here doesn't leave a
        // transaction in an ambiguous state (reads never mutate) — catch
        // it (repo-wide rule: "any long-lived thread must catch panics
        // like spawn_cmd does"), reply with an error, and keep serving
        // subsequent requests rather than parking the whole thread.
        let outcome = panic::catch_unwind(AssertUnwindSafe(|| execute(conn, req.kind)))
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
        ReaderRequestKind::RecentSearches { limit } => {
            crate::search_history::recent(conn, limit).map(ReaderReply::RecentSearches)
        }
        ReaderRequestKind::RecentDocuments { limit } => {
            crate::document::recent_paths(conn, limit).map(ReaderReply::RecentDocuments)
        }
        ReaderRequestKind::RecentCommands { limit } => {
            crate::command_history::recent(conn, limit).map(ReaderReply::RecentCommands)
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
        let uri = crate::conn::memory_uri();
        let bootstrap = crate::conn::open_recovery_store(crate::conn::RecoveryTarget::Memory(&uri))
            .expect("bootstrap shared memdb");

        let handle = spawn(&uri).expect("spawn reader");
        let reply = handle.query(ReaderRequestKind::Ping).expect("ping");
        assert_eq!(reply, ReaderReply::Pong);
        handle.shutdown();

        drop(bootstrap);
    }

    #[test]
    fn get_blob_round_trips_through_the_reader() {
        let uri = crate::conn::memory_uri();
        let bootstrap = crate::conn::open_recovery_store(crate::conn::RecoveryTarget::Memory(&uri))
            .expect("bootstrap shared memdb");
        let hash = crate::blob::put_blob(&bootstrap, b"reader blob content").expect("put blob");

        let handle = spawn(&uri).expect("spawn reader");
        let reply = handle
            .query(ReaderRequestKind::GetBlob { hash })
            .expect("get blob");
        assert_eq!(reply, ReaderReply::Blob(b"reader blob content".to_vec()));
        handle.shutdown();

        drop(bootstrap);
    }

    #[test]
    fn recent_searches_round_trips_through_the_reader() {
        let uri = crate::conn::memory_uri();
        let mut bootstrap =
            crate::conn::open_recovery_store(crate::conn::RecoveryTarget::Memory(&uri))
                .expect("bootstrap shared memdb");

        let base = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1000);
        for (i, query) in ["alpha", "beta"].into_iter().enumerate() {
            let tx = bootstrap.transaction().expect("tx");
            crate::search_history::touch(
                &tx,
                query,
                base + std::time::Duration::from_secs(i as u64 * 10),
            )
            .expect("touch");
            tx.commit().expect("commit");
        }

        let handle = spawn(&uri).expect("spawn reader");
        let reply = handle
            .query(ReaderRequestKind::RecentSearches { limit: 10 })
            .expect("recent searches");
        assert_eq!(
            reply,
            ReaderReply::RecentSearches(vec!["beta".to_string(), "alpha".to_string()])
        );

        let query_handle = handle.as_query();
        let reply2 = query_handle
            .query(ReaderRequestKind::RecentSearches { limit: 1 })
            .expect("recent searches via cloneable handle");
        assert_eq!(
            reply2,
            ReaderReply::RecentSearches(vec!["beta".to_string()])
        );

        // `query_handle` deliberately stays alive across shutdown: a live
        // clone must not keep the reader thread running (shutdown is an
        // explicit sentinel, not sender disconnect), and a query sent
        // afterwards must error instead of blocking.
        handle.shutdown();
        assert!(
            query_handle
                .query(ReaderRequestKind::RecentSearches { limit: 1 })
                .is_err(),
            "query after shutdown must return an error, not block"
        );

        drop(bootstrap);
    }

    #[test]
    fn recent_commands_round_trips_through_the_reader() {
        let uri = crate::conn::memory_uri();
        let mut bootstrap =
            crate::conn::open_recovery_store(crate::conn::RecoveryTarget::Memory(&uri))
                .expect("bootstrap shared memdb");

        let base = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1000);
        for (i, name) in ["save", "quit"].into_iter().enumerate() {
            let tx = bootstrap.transaction().expect("tx");
            crate::command_history::touch(
                &tx,
                name,
                base + std::time::Duration::from_secs(i as u64 * 10),
            )
            .expect("touch");
            tx.commit().expect("commit");
        }

        let handle = spawn(&uri).expect("spawn reader");
        let reply = handle
            .query(ReaderRequestKind::RecentCommands { limit: 10 })
            .expect("recent commands");
        assert_eq!(
            reply,
            ReaderReply::RecentCommands(vec!["quit".to_string(), "save".to_string()])
        );

        let query_handle = handle.as_query();
        let reply2 = query_handle
            .query(ReaderRequestKind::RecentCommands { limit: 1 })
            .expect("recent commands via cloneable handle");
        assert_eq!(
            reply2,
            ReaderReply::RecentCommands(vec!["quit".to_string()])
        );

        handle.shutdown();
        assert!(
            query_handle
                .query(ReaderRequestKind::RecentCommands { limit: 1 })
                .is_err(),
            "query after shutdown must return an error, not block"
        );

        drop(bootstrap);
    }

    /// Pins the `RecentDocuments` request all the way through the real
    /// reader thread — `recent_paths` itself is unit-tested against a bare
    /// `Connection` in `document.rs`, but only this test proves the
    /// `ReaderRequestKind`/`ReaderReply` wiring around it is actually
    /// correct end to end.
    #[test]
    fn recent_documents_round_trips_through_the_reader() {
        let uri = crate::conn::memory_uri();
        let mut bootstrap =
            crate::conn::open_recovery_store(crate::conn::RecoveryTarget::Memory(&uri))
                .expect("bootstrap shared memdb");

        let vfs = rune_vfs::Mem::new();
        let base = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1000);
        crate::document::open_path(
            &mut bootstrap,
            &vfs,
            std::path::Path::new("/doc/a.md"),
            base,
        )
        .expect("open a");
        crate::document::open_path(
            &mut bootstrap,
            &vfs,
            std::path::Path::new("/doc/b.md"),
            base + std::time::Duration::from_secs(10),
        )
        .expect("open b");

        let handle = spawn(&uri).expect("spawn reader");
        let reply = handle
            .query(ReaderRequestKind::RecentDocuments { limit: 10 })
            .expect("recent documents");
        assert_eq!(
            reply,
            ReaderReply::RecentDocuments(vec!["/doc/b.md".to_string(), "/doc/a.md".to_string()])
        );
        handle.shutdown();

        drop(bootstrap);
    }
}
