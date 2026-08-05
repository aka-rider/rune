//! Pins the load-bearing claim `wal.rs` makes but its inline unit tests
//! never actually exercise: that a real process-level death (SIGABRT)
//! leaves a decodable `inflight.rune` behind, while a Rust-level unwind
//! (a panicking test case, which proptest itself catches per-case) does
//! not. Neither can be observed in-process — `catch_unwind` cannot trap a
//! signal, and Drop-on-unwind is exactly the thing under test — so this
//! self-execs the test binary itself (precedent: `crates/rune-cli/tests/
//! launch.rs`), running each `#[ignore]`d child case as its own process
//! and asserting on the parent's view of how the child died.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use rune_fuzz::action::Action;
use rune_fuzz::driver::DOC_PATH;
use rune_fuzz::{script, wal};

const ARTIFACTS_DIR_ENV: &str = "WAL_ABORT_DIR";
const INFLIGHT_NAME: &str = "inflight.rune";

fn armed_script() -> (String, String, Vec<Action>) {
    (
        DOC_PATH.to_string(),
        "hello".to_string(),
        vec![Action::Type("world".to_string())],
    )
}

/// Child process entry point: arms the WAL with a fixed script and then
/// dies by `abort()`. Does nothing when run as an ordinary (non-`--ignored`)
/// test, so a stray `cargo test -- --ignored` sweep of this file is
/// harmless.
#[test]
#[ignore = "child-mode only: invoked by wal_survives_process_abort via self-exec"]
fn child_arm_then_abort() {
    let Ok(dir) = env::var(ARTIFACTS_DIR_ENV) else {
        return;
    };
    let (path, content, actions) = armed_script();
    let _guard = wal::arm(Path::new(&dir), &path, &content, &actions).expect("arm");
    std::process::abort();
}

/// The flipped control: arms the WAL identically, then dies by unwind
/// (a plain panic) instead of a signal. Proves the guard's Drop runs on
/// unwind and removes the file.
#[test]
#[ignore = "child-mode only: invoked by wal_cleaned_on_rust_unwind via self-exec"]
fn child_arm_then_panic() {
    let Ok(dir) = env::var(ARTIFACTS_DIR_ENV) else {
        return;
    };
    let (path, content, actions) = armed_script();
    let _guard = wal::arm(Path::new(&dir), &path, &content, &actions).expect("arm");
    panic!("deliberate unwind for wal_cleaned_on_rust_unwind");
}

struct ScratchDir(PathBuf);

impl ScratchDir {
    fn new(label: &str) -> ScratchDir {
        let dir = env::temp_dir().join(format!(
            "rune-fuzz-wal-abort-{label}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create scratch dir");
        ScratchDir(dir)
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn run_child(dir: &Path, test_name: &str) -> std::process::ExitStatus {
    Command::new(env::current_exe().expect("current test binary path"))
        .args(["--ignored", "--exact", "--test-threads=1", test_name])
        .env(ARTIFACTS_DIR_ENV, dir)
        .status()
        .expect("spawn child process")
}

#[test]
fn wal_survives_process_abort() {
    let scratch = ScratchDir::new("survives-abort");
    let dir = &scratch.0;

    let status = run_child(dir, "child_arm_then_abort");

    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        // 6 is SIGABRT (bits/signum.h): std::process::abort() raises it.
        assert_eq!(
            status.signal(),
            Some(6),
            "child must die to SIGABRT, not exit normally: {status:?}"
        );
    }

    let inflight_path = dir.join(INFLIGHT_NAME);
    let on_disk = fs::read_to_string(&inflight_path)
        .unwrap_or_else(|e| panic!("expected inflight.rune to survive the abort: {e}"));
    let (armed_path, armed_content, armed_actions) = armed_script();
    let (decoded_path, decoded_content, decoded_actions) =
        script::decode(&on_disk).expect("inflight.rune must decode");
    assert_eq!(decoded_path, armed_path);
    assert_eq!(decoded_content, armed_content);
    assert_eq!(decoded_actions, armed_actions);

    let promoted = wal::sweep(dir)
        .expect("sweep must succeed")
        .expect("sweep must promote the leftover inflight file");
    assert!(!inflight_path.exists());
    let promoted_script =
        fs::read_to_string(promoted.join("script.rune")).expect("read promoted script");
    assert_eq!(promoted_script, on_disk);
    let report_text =
        fs::read_to_string(promoted.join("report.txt")).expect("read promoted report");
    assert!(!report_text.is_empty());
}

#[test]
fn wal_cleaned_on_rust_unwind() {
    let scratch = ScratchDir::new("cleaned-unwind");
    let dir = &scratch.0;

    let status = run_child(dir, "child_arm_then_panic");

    assert!(
        !status.success(),
        "child must exit non-zero: the harness reports a failed test"
    );
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(
            status.signal(),
            None,
            "an unwind must exit normally-with-failure, never by signal: {status:?}"
        );
    }

    let inflight_path = dir.join(INFLIGHT_NAME);
    assert!(
        !inflight_path.exists(),
        "the guard's Drop must have removed inflight.rune on unwind"
    );
    assert_eq!(wal::sweep(dir).expect("sweep must succeed"), None);
}
