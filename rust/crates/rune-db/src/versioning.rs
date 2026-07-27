//! Schema versioning by **filename**, never by migration (plan decision 2).
//!
//! A schema-shape change bumps [`SCHEMA_VERSION`] and ships as a new
//! `rune-v{N}.db`; the previous binary's file is left untouched, and a
//! stale binary launched later goes on using its own version's file
//! undisturbed. This is a deliberate departure from Go's `dropIfStale`
//! (which deletes and recreates a file in place on a shape mismatch) — Go
//! could get away with that because only one Go binary version is ever
//! installed at a time; the Rust port's filename scheme instead lets an
//! old-version GC (WP6) reclaim genuinely abandoned files without ever
//! racing a binary that's still using them (see the plan's Risks section,
//! "Old-version GC vs a concurrently launching old binary").
//!
//! # The frozen liveness contract
//!
//! **Every `rune-v*.db` this crate ever produces — past, present, and
//! future — must satisfy the query**
//!
//! ```sql
//! SELECT pid, proc_started_at FROM sessions
//! ```
//!
//! The old-version GC (WP6) runs exactly this query, read-only, against
//! every `rune-v{M}.db` (`M` < current) it finds before touching the file:
//! success and "every session dead" together are its ONLY green light to
//! delete a stale file. A schema change is free to add columns or tables,
//! but the `sessions` table and its `pid`/`proc_started_at` columns may
//! never be renamed, retyped, or dropped — doing so would make every GC
//! implementation from that version onward unable to tell a live old-binary
//! session apart from a dead one, and "on ANY error leave the file and log"
//! (the GC's own safety rule, WP6.S3) means the practical failure mode is
//! merely a leaked file, not data loss — but it is still a contract worth
//! keeping deliberately, not by accident.
//!
//! # Residual risk (plan Risks, R3, accepted)
//!
//! Between an old binary opening `rune-v{M}.db` and inserting its session
//! row, a concurrently starting new binary's GC could observe "no live
//! sessions" and delete the file. WP6's mitigations (mtime > 1 hour, exact
//! filename match, frozen-contract query must succeed) narrow this to a
//! window that requires launching a stale binary against an already-stale
//! (>1h idle) file at the exact moment a newer binary's GC runs — not a
//! supported flow. If hit, the old instance falls back to the degraded
//! `:memory:` open-ladder rung; no user file is ever touched.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use rusqlite::{Connection, OpenFlags};

/// The current schema version. Bump on any shape change (new table, column,
/// index, or constraint) and never in place — see the module doc.
pub const SCHEMA_VERSION: u32 = 1;

/// The filename `rune-v{SCHEMA_VERSION}.db` uses, isolated so tests and the
/// GC (WP6) can match it without recomputing the format string.
pub fn db_file_name(version: u32) -> String {
    format!("rune-v{version}.db")
}

/// The production database path:
/// `$HOME/Library/Application Support/rune/rune-v{SCHEMA_VERSION}.db`
/// (plan decision 1 — one global DB for every instance/workspace; built from
/// `$HOME` since this is a macOS-only app with no `directories` dependency).
///
/// Returns `None` when `$HOME` isn't set — the caller (`store::open`'s
/// production entry point) treats that exactly like any other open-ladder
/// failure and falls back to the degraded in-memory store.
pub fn production_db_path() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("HOME")?;
    if home.is_empty() {
        return None;
    }
    Some(
        std::path::PathBuf::from(home)
            .join("Library")
            .join("Application Support")
            .join("rune")
            .join(db_file_name(SCHEMA_VERSION)),
    )
}

/// How old (by filesystem mtime) an old-version `rune-v{M}.db` file must be
/// before the GC will even consider it — the residual-race mitigation
/// documented above (a stale binary launching against an already-idle file
/// needs well over an hour of coincidence to collide with a GC run).
const MIN_AGE_BEFORE_GC: Duration = Duration::from_secs(3600);

/// Scans `dir` for old-version `rune-v{M}.db` files (`M` < [`SCHEMA_VERSION`])
/// and deletes each one, plus its `-wal`/`-shm` sidecars, once ALL of:
/// - the filename matches `^rune-v(\d+)\.db$` EXACTLY — no other filename in
///   `dir` is ever touched;
/// - its mtime is older than [`MIN_AGE_BEFORE_GC`];
/// - the frozen liveness contract query (see the module doc) succeeds
///   against it, opened READ-ONLY;
/// - `is_alive` reports every `(pid, proc_started_at)` pair it returned as
///   dead.
///
/// Best-effort throughout, exactly like `reaper::reap_dead_sessions`: `dir`
/// not existing, a candidate that fails to open, or a query that errors all
/// just leave that ONE file alone (logged) rather than propagating an error
/// — WP6.S3's own safety rule ("on ANY error leave the file and log") means
/// the worst case of a bug here is a leaked file, never data loss. Called
/// once per `Store::open` (`store.rs`), alongside the dead-session reaper.
pub fn gc_old_versions(dir: &Path, is_alive: &dyn Fn(i64, &str) -> bool) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        maybe_gc_one(&entry.path(), is_alive);
    }
}

fn maybe_gc_one(path: &Path, is_alive: &dyn Fn(i64, &str) -> bool) {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return;
    };
    let Some(version) = parse_old_version_filename(name) else {
        return; // never touch a filename that isn't exactly rune-v<digits>.db
    };
    if version >= SCHEMA_VERSION {
        return; // never the current (or a newer, impossible-but-be-safe) version
    }

    let Ok(metadata) = std::fs::metadata(path) else {
        return;
    };
    let Ok(modified) = metadata.modified() else {
        return;
    };
    match SystemTime::now().duration_since(modified) {
        Ok(age) if age >= MIN_AGE_BEFORE_GC => {}
        _ => return, // too young (or a clock oddity) — leave it
    }

    let sessions = match read_frozen_contract(path) {
        Ok(rows) => rows,
        Err(e) => {
            eprintln!("rune-db: gc: leaving {name} (frozen contract query failed: {e})");
            return;
        }
    };
    if sessions
        .iter()
        .any(|(pid, started_at)| is_alive(*pid, started_at))
    {
        return; // at least one recorded session is still alive — leave it
    }

    delete_old_version_files(path, name);
}

/// Matches EXACTLY `rune-v<digits>.db` (no regex dependency needed for one
/// fixed, simple shape) — returns the parsed version number.
fn parse_old_version_filename(name: &str) -> Option<u32> {
    let digits = name.strip_prefix("rune-v")?.strip_suffix(".db")?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// Opens `path` READ-ONLY and runs exactly the frozen liveness contract
/// query (module doc). Any failure — the file doesn't open, isn't even a
/// SQLite database, or lacks a `sessions` table in this exact shape — is
/// reported to the caller, which leaves the file alone and logs (WP6.S3).
fn read_frozen_contract(path: &Path) -> Result<Vec<(i64, String)>, rusqlite::Error> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut stmt = conn.prepare("SELECT pid, proc_started_at FROM sessions")?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    rows.collect()
}

fn delete_old_version_files(path: &Path, name: &str) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(sidecar_path(path, "-wal"));
    let _ = std::fs::remove_file(sidecar_path(path, "-shm"));
    eprintln!("rune-db: gc: deleted stale {name} (every recorded session is dead)");
}

/// SQLite's WAL/SHM sidecar naming: the suffix is appended to the FULL
/// filename (`rune-v0.db-wal`), never substituted for the `.db` extension.
fn sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    let mut os = path.as_os_str().to_owned();
    os.push(suffix);
    PathBuf::from(os)
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
    use crate::store::Store;

    /// Proves the frozen liveness contract holds for a freshly created
    /// store: exactly the query WP6's GC will run against every past and
    /// future `rune-v*.db`, executed here against a version-1 file this
    /// crate just created.
    #[test]
    fn frozen_liveness_contract_query_succeeds_on_a_fresh_store() {
        let dir = tempfile_dir("versioning-frozen-contract");
        let path = dir.join(db_file_name(SCHEMA_VERSION));

        let (store, warning) = Store::open(
            &path,
            std::sync::Arc::new(rune_vfs::Disk),
            Box::new(|_evt| {}),
        )
        .expect("open store");
        assert!(
            warning.is_none(),
            "fresh writable temp dir must not degrade"
        );
        assert!(!store.degraded());

        // Open a second, independent connection to the same file (mirrors
        // exactly what the WP6 GC does: a fresh read-only look, no Store
        // involved) and run the frozen-contract query verbatim.
        let verify = rusqlite::Connection::open(&path).expect("open verify connection");
        let mut stmt = verify
            .prepare("SELECT pid, proc_started_at FROM sessions")
            .expect("prepare frozen contract query");
        let mut rows = stmt.query([]).expect("run frozen contract query");
        let mut count = 0;
        while let Some(row) = rows.next().expect("row") {
            let _pid: i64 = row.get(0).expect("pid column");
            let _started_at: String = row.get(1).expect("proc_started_at column");
            count += 1;
        }
        assert_eq!(count, 1, "exactly this session's own row must be present");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Local temp-dir helper (kept here rather than pulled in from a test
    /// module elsewhere in the crate — `versioning.rs` is the one module
    /// that needs a real file path rather than an in-memory store).
    fn tempfile_dir(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "rune-db-test-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    /// Creates a fresh SQLite file with the crate's own schema applied and
    /// `sessions` rows seeded directly (bypassing `session::establish_session`
    /// so the test controls the exact `(pid, proc_started_at)` pair, the
    /// same technique `reaper.rs`'s tests use).
    fn fabricate_old_version_db(path: &std::path::Path, sessions: &[(i64, &str)]) {
        let conn = Connection::open(path).expect("create fabricated db");
        crate::schema::apply(&conn).expect("apply schema to fabricated db");
        for (pid, started_at) in sessions {
            conn.execute(
                "INSERT INTO sessions(pid, proc_started_at, opened_at) VALUES(?1, ?2, 'x')",
                rusqlite::params![pid, started_at],
            )
            .expect("seed session");
        }
    }

    fn backdate(path: &std::path::Path, age: Duration) {
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .expect("open for backdate");
        let old = SystemTime::now() - age;
        file.set_modified(old).expect("set_modified");
    }

    /// GC gate (a): dead sessions + old mtime => deleted (file AND both
    /// sidecars).
    #[test]
    fn gc_old_versions_deletes_a_stale_file_when_every_session_is_dead() {
        let dir = tempfile_dir("gc-dead");
        let path = dir.join("rune-v0.db");
        fabricate_old_version_db(&path, &[(111, "started")]);
        backdate(&path, Duration::from_secs(3600 * 2));
        std::fs::write(sidecar_path(&path, "-wal"), b"wal").expect("seed wal sidecar");
        std::fs::write(sidecar_path(&path, "-shm"), b"shm").expect("seed shm sidecar");

        gc_old_versions(&dir, &|_pid, _started_at| false);

        assert!(!path.exists(), "the stale file must be deleted");
        assert!(!sidecar_path(&path, "-wal").exists());
        assert!(!sidecar_path(&path, "-shm").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// GC gate (b): a live-pid session => kept.
    #[test]
    fn gc_old_versions_keeps_a_file_with_a_live_session() {
        let dir = tempfile_dir("gc-live");
        let path = dir.join("rune-v0.db");
        fabricate_old_version_db(&path, &[(222, "started")]);
        backdate(&path, Duration::from_secs(3600 * 2));

        gc_old_versions(&dir, &|pid, _started_at| pid == 222);

        assert!(path.exists(), "a file with a live session must be kept");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// GC gate (c): a garbage schema => kept and logged (never a panic, never
    /// deleted just because the frozen-contract query failed).
    #[test]
    fn gc_old_versions_keeps_a_file_with_a_garbage_schema() {
        let dir = tempfile_dir("gc-garbage");
        let path = dir.join("rune-v0.db");
        let conn = Connection::open(&path).expect("create garbage db");
        conn.execute_batch("CREATE TABLE not_sessions(x INTEGER)")
            .expect("garbage schema");
        drop(conn);
        backdate(&path, Duration::from_secs(3600 * 2));

        gc_old_versions(&dir, &|_pid, _started_at| false);

        assert!(
            path.exists(),
            "a file whose frozen-contract query fails must be left alone"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn gc_old_versions_leaves_a_file_younger_than_the_age_floor() {
        let dir = tempfile_dir("gc-young");
        let path = dir.join("rune-v0.db");
        fabricate_old_version_db(&path, &[(333, "started")]);
        // No backdate: mtime is "now" — too young regardless of liveness.

        gc_old_versions(&dir, &|_pid, _started_at| false);

        assert!(path.exists(), "a too-recent file must be left alone");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn gc_old_versions_ignores_non_matching_filenames() {
        let dir = tempfile_dir("gc-nonmatch");
        let path = dir.join("backup-rune-v0.db");
        fabricate_old_version_db(&path, &[(444, "started")]);
        backdate(&path, Duration::from_secs(3600 * 2));

        gc_old_versions(&dir, &|_pid, _started_at| false);

        assert!(
            path.exists(),
            "a non-matching filename must never be touched"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn gc_old_versions_never_touches_the_current_version_file() {
        let dir = tempfile_dir("gc-current");
        let path = dir.join(db_file_name(SCHEMA_VERSION));
        fabricate_old_version_db(&path, &[(555, "started")]);
        backdate(&path, Duration::from_secs(3600 * 2));

        gc_old_versions(&dir, &|_pid, _started_at| false);

        assert!(path.exists(), "the current-version file must never be GC'd");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
