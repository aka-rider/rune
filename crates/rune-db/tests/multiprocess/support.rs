//! Shared parent/child utilities for the multiprocess scenarios:
//! temp-directory allocation, marker-file rendezvous
//! (bounded poll-until-condition, never a fixed-length pacing sleep — repo
//! convention), and re-exec-self child spawning.

use std::env;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, params};

/// A hang safety net, not a pacing bound: every rendezvous in these
/// scenarios completes in well under a second on a healthy run, and a
/// liveness-aware wait already fails fast the moment a child dies. This
/// deadline only ever fires against a genuinely stuck, still-alive child —
/// it must never be tuned down to make a scenario run "faster".
pub(crate) const MARKER_SAFETY_DEADLINE: Duration = Duration::from_secs(120);

/// Poll cadence shared by every marker-rendezvous loop below.
pub(crate) const MARKER_POLL_INTERVAL: Duration = Duration::from_millis(5);

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
        std::thread::sleep(MARKER_POLL_INTERVAL);
    }
}

/// Like a bounded poll-until-condition wait for `markers`, but also holds
/// the spawned children's own handles: on every poll it checks whether any
/// of them has already exited while any marker is still missing, and if so
/// panics immediately with that child's exit status and captured stderr — a
/// child must stay alive until every marker is present, so its death before
/// that point is a real defect, not something the deadline should have to
/// wait out. The deadline itself only ever fires against a live-but-hung
/// child.
pub(crate) fn wait_ready_or_child_death(
    children: &mut Vec<Child>,
    markers: &[PathBuf],
    deadline: Duration,
) {
    let start = Instant::now();
    loop {
        if markers.iter().all(|p| p.exists()) {
            return;
        }
        for i in 0..children.len() {
            if let Ok(Some(status)) = children[i].try_wait() {
                let dead = children.swap_remove(i);
                let output = dead
                    .wait_with_output()
                    .unwrap_or_else(|e| panic!("collect output of dead child: {e}"));
                panic!(
                    "child exited ({status}) before all ready markers appeared, markers: \
                     {markers:?}, stderr: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
        }
        if start.elapsed() > deadline {
            panic!("timed out after {deadline:?} waiting for markers: {markers:?}");
        }
        std::thread::sleep(MARKER_POLL_INTERVAL);
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
