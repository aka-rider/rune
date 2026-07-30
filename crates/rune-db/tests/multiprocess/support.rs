//! Shared parent/child utilities for the multiprocess scenarios (§1.6
//! budget split): temp-directory allocation, marker-file rendezvous
//! (bounded poll-until-condition, never a fixed-length pacing sleep — repo
//! convention), and re-exec-self child spawning.

use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, params};

pub(crate) fn temp_dir(label: &str) -> PathBuf {
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

pub(crate) fn touch(path: &Path) {
    std::fs::write(path, b"1").unwrap_or_else(|e| panic!("touch {path:?}: {e}"));
}

/// Bounded poll-until-condition with a deadline — never a fixed-duration
/// pacing sleep (repo convention).
pub(crate) fn wait_for_path(path: &Path, deadline: Duration) {
    let start = Instant::now();
    while !path.exists() {
        if start.elapsed() > deadline {
            panic!("timed out after {deadline:?} waiting for {path:?}");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

pub(crate) fn wait_for_all(paths: &[PathBuf], deadline: Duration) {
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

pub(crate) fn helper_exe() -> PathBuf {
    env::current_exe().expect("current exe")
}

pub(crate) fn spawn_helper(role: &str, envs: &[(&str, String)]) -> std::process::Child {
    let mut cmd = Command::new(helper_exe());
    cmd.arg("--exact")
        .arg("helper_entrypoint")
        .arg("--nocapture");
    cmd.env(crate::ROLE_ENV, role);
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
/// race-free by construction, so the scenarios can focus on the real
/// cross-process behavior WP6 is meant to prove rather than document
/// identity resolution (already covered by WP4's own tests).
pub(crate) fn seed_schema_and_docs(path: &Path, n: usize) -> Vec<i64> {
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
