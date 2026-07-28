//! Multiprocess integration tests (plan WP6.S4, R4's resolution: "the
//! multiprocess tests in WP6 must spawn real child processes" — same-process
//! threads share SQLite's in-process lock table and never exercise the real
//! cross-process locking this crate depends on).
//!
//! # The re-exec-self pattern
//!
//! Each scenario test below spawns `std::env::current_exe()` (THIS test
//! binary) as a child process with `--exact helper_entrypoint --nocapture`
//! and a `RUNE_DB_HELPER=<role>` environment variable. `cargo test`'s own
//! harness then runs ONLY [`helper_entrypoint`] in the child (no custom
//! `main`/`#[ctor]` needed); that test reads `RUNE_DB_HELPER`, dispatches to
//! the matching role in `helper`, and the role function itself calls
//! `std::process::exit` when it succeeds (a role panic — a `.expect`
//! failure — makes `cargo test` report that one test as failed, which is
//! exactly "the child process exited non-zero").
//!
//! When `RUNE_DB_HELPER` is unset (an ordinary `cargo test` run),
//! `helper_entrypoint` is a no-op passing test.
//!
//! # No wall-clock pacing
//!
//! Every rendezvous between parent and children is a filesystem marker file
//! plus a bounded poll-until-condition with a deadline (never a fixed-length
//! sleep used to "probably" order events) — repo convention (`CLAUDE.md`:
//! "never order events with wall-clock sleeps").

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, params};

use rune_core::buffer::AppliedEdit;
use rune_db::{DbEvent, OnEvent, Store};

const ROLE_ENV: &str = "RUNE_DB_HELPER";

// ---------------------------------------------------------------------
// Shared parent/child utilities
// ---------------------------------------------------------------------

fn temp_dir(label: &str) -> PathBuf {
    let dir = env::temp_dir().join(format!(
        "rune-db-mp-{label}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn touch(path: &Path) {
    std::fs::write(path, b"1").unwrap_or_else(|e| panic!("touch {path:?}: {e}"));
}

/// Bounded poll-until-condition with a deadline — never a fixed-duration
/// pacing sleep (repo convention).
fn wait_for_path(path: &Path, deadline: Duration) {
    let start = Instant::now();
    while !path.exists() {
        if start.elapsed() > deadline {
            panic!("timed out after {deadline:?} waiting for {path:?}");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn wait_for_all(paths: &[PathBuf], deadline: Duration) {
    let start = Instant::now();
    loop {
        if paths.iter().all(|p| p.exists()) {
            return;
        }
        if start.elapsed() > deadline {
            panic!("timed out after {deadline:?} waiting for markers: {paths:?}");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn helper_exe() -> PathBuf {
    env::current_exe().expect("current exe")
}

fn spawn_helper(role: &str, envs: &[(&str, String)]) -> std::process::Child {
    let mut cmd = Command::new(helper_exe());
    cmd.arg("--exact")
        .arg("helper_entrypoint")
        .arg("--nocapture");
    cmd.env(ROLE_ENV, role);
    for (k, v) in envs {
        cmd.env(k, v);
    }
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.spawn()
        .unwrap_or_else(|e| panic!("spawn helper role {role}: {e}"))
}

/// Applies the crate's schema and seeds `n` document rows directly via a raw
/// connection, BEFORE any child is spawned — setup is sequential and
/// race-free by construction, so the scenarios below can focus on the real
/// cross-process behavior WP6 is meant to prove rather than document
/// identity resolution (already covered by WP4's own tests).
fn seed_schema_and_docs(path: &Path, n: usize) -> Vec<i64> {
    let conn = Connection::open(path).expect("create db for seeding");
    conn.execute_batch(rune_db::SCHEMA).expect("apply schema");
    let mut ids = Vec::with_capacity(n);
    for i in 0..n {
        conn.execute(
            "INSERT INTO documents(path, created_at, last_seen_at) VALUES (?1, 'x', 'x')",
            params![format!("/mp-doc-{i}.md")],
        )
        .expect("seed doc");
        ids.push(conn.last_insert_rowid());
    }
    ids
}

// ---------------------------------------------------------------------
// Dispatch guard — the ONE test the re-exec'd child ever runs
// ---------------------------------------------------------------------

/// When `RUNE_DB_HELPER` is unset (every normal `cargo test` run), a no-op
/// passing test. When a scenario below spawns this same binary with
/// `--exact helper_entrypoint --nocapture` and `RUNE_DB_HELPER` set, runs
/// the requested role and exits the process itself.
#[test]
fn helper_entrypoint() {
    let Ok(role) = env::var(ROLE_ENV) else {
        return;
    };
    match role.as_str() {
        "append_storm" => helper::append_storm(),
        "append_storm_checkpoint" => helper::append_storm_checkpoint(),
        "race_open" => helper::race_open(),
        "race_close" => helper::race_close(),
        other => {
            eprintln!("multiprocess helper: unknown role {other}");
            std::process::exit(2);
        }
    }
}

mod helper {
    use super::*;

    fn env_var(name: &str) -> String {
        env::var(name).unwrap_or_else(|_| panic!("missing required env var {name}"))
    }

    fn db_path() -> PathBuf {
        PathBuf::from(env_var("RUNE_DB_PATH"))
    }

    fn open_store(path: &Path, on_event: OnEvent) -> Store {
        let (store, warning) =
            Store::open(path, Arc::new(rune_vfs::Disk), on_event).expect("child: open store");
        assert!(
            warning.is_none(),
            "child: must not degrade against a real writable temp path"
        );
        assert!(!store.degraded());
        store
    }

    fn expect_ok(rx: &mpsc::Receiver<DbEvent>, id: u64) {
        match rx.recv_timeout(Duration::from_secs(30)) {
            Ok(DbEvent::Ok { id: got, .. }) if got == id => {}
            Ok(other) => panic!("expected Ok(id:{id}), got {other:?}"),
            Err(e) => panic!("timed out waiting for ack of op {id}: {e}"),
        }
    }

    /// Role (a): append `RUNE_DB_COUNT` edits to `RUNE_DB_DOC_ID`, waiting
    /// for each ack before sending the next (read-your-writes, one writer
    /// at a time from THIS process's own perspective — the other sibling
    /// children racing the SAME db file is exactly what the scenario is
    /// testing). Synchronizes its start with its siblings via a
    /// ready/go marker handshake so the storm genuinely overlaps in time.
    pub fn append_storm() {
        let path = db_path();
        let doc_id: i64 = env_var("RUNE_DB_DOC_ID").parse().expect("doc id");
        let count: usize = env_var("RUNE_DB_COUNT").parse().expect("count");
        let ready = PathBuf::from(env_var("RUNE_DB_READY_MARKER"));
        let go = PathBuf::from(env_var("RUNE_DB_GO_MARKER"));

        let (tx, rx) = mpsc::channel::<DbEvent>();
        let on_event: OnEvent = Box::new(move |evt| {
            let _ = tx.send(evt);
        });
        let store = open_store(&path, on_event);

        touch(&ready);
        wait_for_path(&go, Duration::from_secs(30));

        for i in 0..count {
            let edit = AppliedEdit {
                start: 0,
                end: 0,
                deleted: String::new(),
                insert: format!("{i} "),
            };
            let id = store
                .append_edit(doc_id, &[edit], &[], &[])
                .expect("enqueue append");
            expect_ok(&rx, id);
        }

        store.shutdown();
        std::process::exit(0);
    }

    /// Role (c): like `append_storm`, but after the `RUNE_DB_CHECKPOINT`-th
    /// committed append it writes its own session id and a checkpoint
    /// marker, then BLOCKS waiting for a release marker the parent never
    /// writes — the parent SIGKILLs this process while it is blocked here,
    /// giving a deterministic, race-free "killed after exactly N committed
    /// batches" instant (no window between "read progress" and "issue
    /// kill").
    pub fn append_storm_checkpoint() {
        let path = db_path();
        let doc_id: i64 = env_var("RUNE_DB_DOC_ID").parse().expect("doc id");
        let count: usize = env_var("RUNE_DB_COUNT").parse().expect("count");
        let checkpoint: usize = env_var("RUNE_DB_CHECKPOINT").parse().expect("checkpoint");
        let session_marker = PathBuf::from(env_var("RUNE_DB_SESSION_MARKER"));
        let checkpoint_marker = PathBuf::from(env_var("RUNE_DB_CHECKPOINT_MARKER"));
        let release_marker = PathBuf::from(env_var("RUNE_DB_RELEASE_MARKER"));

        let (tx, rx) = mpsc::channel::<DbEvent>();
        let on_event: OnEvent = Box::new(move |evt| {
            let _ = tx.send(evt);
        });
        let store = open_store(&path, on_event);
        std::fs::write(&session_marker, store.session_id().to_string())
            .expect("write session marker");

        for i in 0..count {
            let edit = AppliedEdit {
                start: 0,
                end: 0,
                deleted: String::new(),
                insert: format!("{i} "),
            };
            let id = store
                .append_edit(doc_id, &[edit], &[], &[])
                .expect("enqueue append");
            expect_ok(&rx, id);

            if i + 1 == checkpoint {
                touch(&checkpoint_marker);
                // Safety-net deadline only: the scenario that spawns this
                // role always kills the process long before this elapses.
                wait_for_path(&release_marker, Duration::from_secs(60));
            }
        }

        store.shutdown();
        std::process::exit(0);
    }

    /// Role (b): race `Store::open` itself against a fresh (not yet
    /// existing) path, synchronized with the sibling via ready/go markers.
    pub fn race_open() {
        let path = db_path();
        let ready = PathBuf::from(env_var("RUNE_DB_READY_MARKER"));
        let go = PathBuf::from(env_var("RUNE_DB_GO_MARKER"));
        let opened_marker = PathBuf::from(env_var("RUNE_DB_OPENED_MARKER"));

        touch(&ready);
        wait_for_path(&go, Duration::from_secs(30));

        let store = open_store(&path, Box::new(|_evt| {}));
        std::fs::write(&opened_marker, store.session_id().to_string())
            .expect("write opened marker");
        store.shutdown();
        std::process::exit(0);
    }

    /// Role (d): open, then force this session to see every OTHER session
    /// as dead (`Store::set_liveness_check`) so its own shutdown
    /// unconditionally attempts a TRUNCATE checkpoint regardless of whether
    /// the real sibling process has actually exited yet — synchronized with
    /// the sibling so both call `shutdown` at nearly the same instant,
    /// deterministically forcing a genuine TRUNCATE race between two real
    /// OS processes.
    pub fn race_close() {
        let path = db_path();
        let ready = PathBuf::from(env_var("RUNE_DB_READY_MARKER"));
        let go = PathBuf::from(env_var("RUNE_DB_GO_MARKER"));

        let store = open_store(&path, Box::new(|_evt| {}));
        store.set_liveness_check(Arc::new(|_pid, _started_at| false));

        touch(&ready);
        wait_for_path(&go, Duration::from_secs(30));

        store.shutdown();
        std::process::exit(0);
    }
}

// ---------------------------------------------------------------------
// Scenario (a): 4 children append-storm one doc each concurrently
// ---------------------------------------------------------------------

#[test]
fn four_children_append_storm_one_doc_each_all_ack_ok_with_exact_event_counts() {
    let dir = temp_dir("append-storm");
    let path = dir.join("rune-v1.db");
    let doc_ids = seed_schema_and_docs(&path, 4);
    let count = 25usize;
    let go = dir.join("go");

    let mut children = Vec::new();
    let mut readies = Vec::new();
    for (i, doc_id) in doc_ids.iter().enumerate() {
        let ready = dir.join(format!("ready-{i}"));
        readies.push(ready.clone());
        children.push(spawn_helper(
            "append_storm",
            &[
                ("RUNE_DB_PATH", path.display().to_string()),
                ("RUNE_DB_DOC_ID", doc_id.to_string()),
                ("RUNE_DB_COUNT", count.to_string()),
                ("RUNE_DB_READY_MARKER", ready.display().to_string()),
                ("RUNE_DB_GO_MARKER", go.display().to_string()),
            ],
        ));
    }

    wait_for_all(&readies, Duration::from_secs(30));
    touch(&go);

    for child in children {
        let output = child.wait_with_output().expect("wait child");
        assert!(
            output.status.success(),
            "append_storm child failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let verify = Connection::open(&path).expect("open verify connection");
    for doc_id in &doc_ids {
        let n: i64 = verify
            .query_row(
                "SELECT COUNT(*) FROM events WHERE doc_id=?1",
                params![doc_id],
                |r| r.get(0),
            )
            .expect("count events");
        assert_eq!(
            n, count as i64,
            "doc {doc_id} must have exactly {count} events"
        );
    }
    let total: i64 = verify
        .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
        .expect("count total events");
    assert_eq!(total, 4 * count as i64);

    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------
// Scenario (b): two children race Store::open on a fresh path
// ---------------------------------------------------------------------

#[test]
fn two_children_race_store_open_on_a_fresh_path_apply_schema_once_both_get_sessions() {
    let dir = temp_dir("race-open");
    let path = dir.join("rune-v1.db"); // does NOT exist yet
    let go = dir.join("go");

    let mut children = Vec::new();
    let mut readies = Vec::new();
    for i in 0..2 {
        let ready = dir.join(format!("ready-{i}"));
        let opened = dir.join(format!("opened-{i}"));
        readies.push(ready.clone());
        children.push(spawn_helper(
            "race_open",
            &[
                ("RUNE_DB_PATH", path.display().to_string()),
                ("RUNE_DB_READY_MARKER", ready.display().to_string()),
                ("RUNE_DB_GO_MARKER", go.display().to_string()),
                ("RUNE_DB_OPENED_MARKER", opened.display().to_string()),
            ],
        ));
    }

    wait_for_all(&readies, Duration::from_secs(30));
    touch(&go);

    for child in children {
        let output = child.wait_with_output().expect("wait child");
        assert!(
            output.status.success(),
            "race_open child failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let verify = Connection::open(&path).expect("open verify connection");
    let integrity: String = verify
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .expect("integrity check");
    assert_eq!(integrity, "ok");
    let sessions: i64 = verify
        .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))
        .expect("count sessions");
    assert_eq!(
        sessions, 2,
        "both racing opens must each get their own session row"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------
// Scenario (c): child SIGKILLed mid-storm
// ---------------------------------------------------------------------

#[test]
fn child_sigkilled_mid_storm_recovers_at_last_committed_batch_and_reaper_reclaims() {
    let dir = temp_dir("kill-mid-storm");
    let path = dir.join("rune-v1.db");
    let doc_ids = seed_schema_and_docs(&path, 1);
    let doc_id = doc_ids[0];
    let count = 200usize;
    let checkpoint = 50usize;

    let session_marker = dir.join("session");
    let checkpoint_marker = dir.join("checkpoint");
    let release_marker = dir.join("release"); // intentionally never written

    let mut child = spawn_helper(
        "append_storm_checkpoint",
        &[
            ("RUNE_DB_PATH", path.display().to_string()),
            ("RUNE_DB_DOC_ID", doc_id.to_string()),
            ("RUNE_DB_COUNT", count.to_string()),
            ("RUNE_DB_CHECKPOINT", checkpoint.to_string()),
            (
                "RUNE_DB_SESSION_MARKER",
                session_marker.display().to_string(),
            ),
            (
                "RUNE_DB_CHECKPOINT_MARKER",
                checkpoint_marker.display().to_string(),
            ),
            (
                "RUNE_DB_RELEASE_MARKER",
                release_marker.display().to_string(),
            ),
        ],
    );

    wait_for_path(&checkpoint_marker, Duration::from_secs(30));
    child.kill().expect("sigkill child");
    let _ = child.wait_with_output();

    let killed_session_id: i64 = std::fs::read_to_string(&session_marker)
        .expect("read session marker")
        .trim()
        .parse()
        .expect("parse session id");

    // Parent "reopens": a fresh Store::open against the same path.
    let (tx, rx) = mpsc::channel::<DbEvent>();
    let on_event: OnEvent = Box::new(move |evt| {
        let _ = tx.send(evt);
    });
    let (store, warning) =
        Store::open(&path, Arc::new(rune_vfs::Disk), on_event).expect("reopen store");
    assert!(warning.is_none());
    assert!(!store.degraded());

    // recover_document is scoped to a SESSION's own current_seq (never
    // touched by plain append_edit, so it defaults to "at head") — calling
    // it with the KILLED session's own id replays exactly its own committed
    // events, which is exactly the content the child had journaled before
    // being killed.
    let mut verify = Connection::open(&path).expect("verify connection");
    let recovered = {
        let tx = verify.transaction().expect("tx");
        let content =
            rune_db::recover_document(&tx, killed_session_id, doc_id).expect("recover_document");
        tx.commit().expect("commit");
        content
    };
    // Each edit inserts at position 0 (`start: 0, end: 0`), so content
    // accumulates with the LATEST insert first — the committed prefix reads
    // in descending order.
    let expected: String = (0..checkpoint).rev().map(|i| format!("{i} ")).collect();
    assert_eq!(
        recovered, expected,
        "recovered content must match exactly the committed prefix"
    );

    let killed_events: i64 = verify
        .query_row(
            "SELECT COUNT(*) FROM events WHERE session_id=?1",
            params![killed_session_id],
            |r| r.get(0),
        )
        .expect("count killed session events");
    assert_eq!(
        killed_events, checkpoint as i64,
        "exactly `checkpoint` events must have committed before the kill"
    );

    // A new session appends past the dead one, establishing itself as the
    // new most-recent toucher of doc_id.
    let edit = AppliedEdit {
        start: recovered.len(),
        end: recovered.len(),
        deleted: String::new(),
        insert: "NEW".to_string(),
    };
    let id = store
        .append_edit(doc_id, &[edit], &[], &[])
        .expect("enqueue append");
    match rx.recv_timeout(Duration::from_secs(30)) {
        Ok(DbEvent::Ok { id: got, .. }) if got == id => {}
        other => panic!("expected append ack, got {other:?}"),
    }
    store.shutdown();

    // The killed child's real pid genuinely no longer exists, so the REAL
    // `is_process_alive` naturally reports it dead — no test override
    // needed. The reaper only reclaims a dead session once it is no longer
    // the most-recent toucher, which the append above just ensured.
    let mut reap_conn = Connection::open(&path).expect("reap connection");
    rune_db::reap_dead_sessions(&mut reap_conn, &rune_db::is_process_alive).expect("reap");

    let killed_events_after_reap: i64 = reap_conn
        .query_row(
            "SELECT COUNT(*) FROM events WHERE session_id=?1",
            params![killed_session_id],
            |r| r.get(0),
        )
        .expect("count killed session events after reap");
    assert_eq!(
        killed_events_after_reap, 0,
        "the killed, now-superseded session's footprint must be reaped"
    );

    let killed_session_row_exists: bool = reap_conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sessions WHERE id=?1)",
            params![killed_session_id],
            |r| r.get(0),
        )
        .expect("check sessions row");
    assert!(
        killed_session_row_exists,
        "the sessions row itself must never be deleted"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------
// Scenario (d): two stores closing simultaneously
// ---------------------------------------------------------------------

#[test]
fn two_stores_closing_simultaneously_surface_no_error_despite_truncate_contention() {
    let dir = temp_dir("race-close");
    let path = dir.join("rune-v1.db");
    let _ = seed_schema_and_docs(&path, 0);
    let go = dir.join("go");

    let mut children = Vec::new();
    let mut readies = Vec::new();
    for i in 0..2 {
        let ready = dir.join(format!("ready-{i}"));
        readies.push(ready.clone());
        children.push(spawn_helper(
            "race_close",
            &[
                ("RUNE_DB_PATH", path.display().to_string()),
                ("RUNE_DB_READY_MARKER", ready.display().to_string()),
                ("RUNE_DB_GO_MARKER", go.display().to_string()),
            ],
        ));
    }

    wait_for_all(&readies, Duration::from_secs(30));
    touch(&go);

    for child in children {
        let output = child.wait_with_output().expect("wait child");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "race_close child failed: {stderr}");
        assert!(
            !stderr.contains("panicked"),
            "child stderr shows a panic despite BUSY-class TRUNCATE contention being \
             expected and swallowed by design: {stderr}"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}
