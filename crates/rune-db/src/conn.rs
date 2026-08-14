use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use rusqlite::{Connection, OpenFlags};

use crate::Error;

#[derive(Clone, Copy)]
pub(crate) enum RecoveryTarget<'a> {
    File(&'a Path),
    Memory(&'a str),
}

pub(crate) fn open_recovery_store(target: RecoveryTarget) -> Result<Connection, Error> {
    let conn = match target {
        RecoveryTarget::File(path) => Connection::open(path)?,
        RecoveryTarget::Memory(uri) => Connection::open_with_flags(uri, memory_open_flags())?,
    };
    apply_pragmas(&conn)?;
    if let RecoveryTarget::File(_) = target {
        verify_wal_mode(&conn)?;
    }
    crate::schema::apply(&conn)?;
    Ok(conn)
}

pub(crate) fn open_read_replica(target: &str) -> Result<Connection, Error> {
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY
        | OpenFlags::SQLITE_OPEN_NO_MUTEX
        | OpenFlags::SQLITE_OPEN_URI;
    let conn = Connection::open_with_flags(target, flags)?;
    apply_pragmas(&conn)?;
    Ok(conn)
}

pub(crate) fn open_frozen_contract_probe(path: &Path) -> Result<Connection, rusqlite::Error> {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
}

#[cfg(any(test, feature = "test-support"))]
pub(crate) fn open_raw(path: &Path) -> Result<Connection, Error> {
    Ok(Connection::open(path)?)
}

pub(crate) fn memory_uri() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!(
        "file:rune-db-mem-{}-{n}?mode=memory&cache=shared",
        std::process::id()
    )
}

fn memory_open_flags() -> OpenFlags {
    OpenFlags::SQLITE_OPEN_READ_WRITE
        | OpenFlags::SQLITE_OPEN_CREATE
        | OpenFlags::SQLITE_OPEN_URI
        | OpenFlags::SQLITE_OPEN_NO_MUTEX
}

fn apply_pragmas(conn: &Connection) -> Result<(), Error> {
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "journal_size_limit", 67_108_864i64)?;
    conn.pragma_update(None, "wal_autocheckpoint", 1000i64)?;
    Ok(())
}

fn verify_wal_mode(conn: &Connection) -> Result<(), Error> {
    let mode: String =
        conn.pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get(0))?;
    if mode.eq_ignore_ascii_case("wal") {
        Ok(())
    } else {
        Err(Error::WalModeUnavailable(mode))
    }
}

#[cfg(feature = "test-support")]
pub fn open_recovery_store_at_path_for_test(path: &Path) -> Result<Connection, Error> {
    open_recovery_store(RecoveryTarget::File(path))
}

#[cfg(feature = "test-support")]
pub fn open_raw_connection_at_path_for_test(path: &Path) -> Result<Connection, Error> {
    open_raw(path)
}

#[cfg(feature = "test-support")]
pub fn fresh_memory_uri_for_test() -> String {
    memory_uri()
}

#[cfg(test)]
#[allow(clippy::panic)]
pub(crate) fn test_temp_dir(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "rune-db-conn-test-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("create temp dir: {e}"));
    dir
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn recovery_store_reads_back_foreign_keys_on() {
        let uri = memory_uri();
        let conn = open_recovery_store(RecoveryTarget::Memory(&uri)).expect("open recovery store");
        let fk: i64 = conn
            .pragma_query_value(None, "foreign_keys", |row| row.get(0))
            .expect("read foreign_keys pragma");
        assert_eq!(fk, 1);
    }

    #[test]
    fn recovery_store_file_backed_is_wal_mode() {
        let dir = test_temp_dir("recovery-wal");
        let path = dir.join("rune-v1.db");
        let conn = open_recovery_store(RecoveryTarget::File(&path)).expect("open recovery store");
        let mode: String = conn
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .expect("read journal_mode");
        assert_eq!(mode, "wal");
        drop(conn);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn raw_connection_has_no_schema() {
        let dir = test_temp_dir("raw-no-schema");
        let path = dir.join("rune-v1.db");
        let conn = open_raw(&path).expect("open raw connection");
        let err = conn
            .query_row(
                "SELECT COUNT(*) FROM sessions",
                [],
                |row: &rusqlite::Row| row.get::<_, i64>(0),
            )
            .expect_err("raw connection must have no schema applied");
        assert!(matches!(err, rusqlite::Error::SqliteFailure(_, _)));
        drop(conn);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn memory_uri_is_process_unique_across_calls() {
        let a = memory_uri();
        let b = memory_uri();
        assert_ne!(a, b);
    }
}
