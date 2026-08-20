use std::panic::{self, AssertUnwindSafe};
use std::sync::mpsc::{self, Sender};
use std::thread;

use rusqlite::Connection;

use crate::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReaderRequestKind {
    Ping,
    GetBlob { hash: String },
    RecentSearches { limit: u32 },
    RecentDocuments { limit: u32 },
    RecentCommands { limit: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReaderReply {
    Pong,
    Blob(Vec<u8>),
    RecentSearches(Vec<String>),
    RecentDocuments(Vec<String>),
    RecentCommands(Vec<String>),
}

struct Request {
    kind: ReaderRequestKind,
    reply: Sender<Result<ReaderReply, Error>>,
}

enum Msg {
    Query(Request),
    Shutdown,
}

pub struct ReaderHandle {
    sender: Sender<Msg>,
    thread: Option<thread::JoinHandle<()>>,
}

impl ReaderHandle {
    pub fn query(&self, kind: ReaderRequestKind) -> Result<ReaderReply, Error> {
        query_over(&self.sender, kind)
    }

    pub fn shutdown(self) {
        let ReaderHandle { sender, thread } = self;
        let _ = sender.send(Msg::Shutdown);
        drop(sender);
        if let Some(thread) = thread {
            let _ = thread.join();
        }
    }

    pub fn as_query(&self) -> ReaderQuery {
        ReaderQuery(self.sender.clone())
    }
}

#[derive(Clone)]
pub struct ReaderQuery(Sender<Msg>);

impl ReaderQuery {
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

/// A read-only SQLite connection cannot create tables; `path` must already have its schema applied.
pub fn spawn(path: &str) -> Result<ReaderHandle, Error> {
    let conn = crate::conn::open_read_replica(path)?;

    let (sender, receiver) = mpsc::channel::<Msg>();
    let thread = thread::spawn(move || reader_loop(&conn, &receiver));
    Ok(ReaderHandle {
        sender,
        thread: Some(thread),
    })
}

fn reader_loop(conn: &Connection, receiver: &mpsc::Receiver<Msg>) {
    while let Ok(msg) = receiver.recv() {
        let req = match msg {
            Msg::Query(req) => req,
            Msg::Shutdown => return,
        };
        let outcome = panic::catch_unwind(AssertUnwindSafe(|| execute(conn, req.kind)))
            .unwrap_or_else(|_| Err(Error::ReaderGone));
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
