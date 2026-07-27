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

        let (store, warning) = Store::open(&path, Box::new(|_evt| {})).expect("open store");
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
}
