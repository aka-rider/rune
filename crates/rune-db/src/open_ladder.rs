//! The open ladder `Store::open` runs against a
//! file path, plus the two connection-opening primitives it bottoms out
//! at. Split out of `store.rs` — see that module's doc comment for
//! the ladder's rungs.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use rusqlite::{Connection, OpenFlags};

use crate::Error;
use crate::store::{DEGRADED_WARNING, apply_connection_pragmas, set_wal_mode_verified};

pub(crate) struct LadderResult {
    pub(crate) writer_conn: Connection,
    /// What the reader thread opens: a plain file path for a file-backed
    /// store, or the same `cache=shared` memory URI the writer just created
    /// for a degraded one.
    pub(crate) reader_target: String,
    pub(crate) warning: Option<String>,
}

pub(crate) fn open_ladder(path: &Path) -> Result<LadderResult, Error> {
    // `reader_target` must round-trip through UTF-8 (A4/[rune-db 6] — the
    // same checked conversion every persisted path goes through, not
    // `to_string_lossy`): a mangled reader target would open the reader
    // thread against a DIFFERENT path than the writer's, silently. A
    // conversion failure here degrades to the next rung exactly like an
    // open failure would — `open_ladder` never hard-fails except at the
    // final in-memory rung.
    if let Ok(conn) = open_file_backed(path)
        && let Ok(reader_target) = crate::paths::to_db_string(path)
    {
        return Ok(LadderResult {
            writer_conn: conn,
            reader_target,
            warning: None,
        });
    }

    if let Some(parent) = path.parent()
        && std::fs::create_dir_all(parent).is_ok()
        && let Ok(conn) = open_file_backed(path)
        && let Ok(reader_target) = crate::paths::to_db_string(path)
    {
        return Ok(LadderResult {
            writer_conn: conn,
            reader_target,
            warning: None,
        });
    }

    let uri = memory_uri();
    let conn = open_memory_backed(&uri)?;
    Ok(LadderResult {
        writer_conn: conn,
        reader_target: uri,
        warning: Some(DEGRADED_WARNING.to_string()),
    })
}

fn open_file_backed(path: &Path) -> Result<Connection, Error> {
    let conn = Connection::open(path)?;
    apply_connection_pragmas(&conn)?;
    set_wal_mode_verified(&conn)?;
    crate::schema::apply(&conn)?;
    Ok(conn)
}

pub(crate) fn open_memory_backed(uri: &str) -> Result<Connection, Error> {
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
        | OpenFlags::SQLITE_OPEN_CREATE
        | OpenFlags::SQLITE_OPEN_URI
        | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = Connection::open_with_flags(uri, flags)?;
    apply_connection_pragmas(&conn)?;
    // journal_mode=WAL is a documented no-op for :memory: databases (falls
    // back to "memory" journaling) — nothing to verify here, unlike the
    // file-backed rung.
    crate::schema::apply(&conn)?;
    Ok(conn)
}

/// A process-unique `cache=shared` in-memory database name, so the writer
/// and reader connections of ONE degraded `Store` see the same data while
/// two independent (degraded or explicitly in-memory) `Store`s never
/// collide with each other.
pub(crate) fn memory_uri() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!(
        "file:rune-db-mem-{}-{n}?mode=memory&cache=shared",
        std::process::id()
    )
}
