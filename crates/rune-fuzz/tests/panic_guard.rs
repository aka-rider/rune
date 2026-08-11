//! Pins the harness's own failure mode: a panic raised on a driver path
//! that is not `update` — the display-pipeline checkers, session setup —
//! must come back as a recorded `NO-PANIC` violation with an artifact
//! bundle behind it, never as an unwind that escapes the harness and
//! leaves the operator a stderr line and a seed.
//!
//! The stderr half cannot be observed in-process (libtest owns the
//! capture), so it uses the same parent/child self-exec shape
//! `wal_abort.rs` does.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use rune_fuzz::action::Action;
use rune_fuzz::driver::{self, DOC_PATH};
use rune_fuzz::invariant::Violation;
use rune_fuzz::{fault, guard, report};

const PANIC_TEXT: &str = "forced emitter panic on the sync-idempotent path";

fn session() -> (String, Vec<Action>) {
    ("hello\nworld\n".to_string(), vec![Action::Type("x".into())])
}

struct ScratchDir(PathBuf);

impl ScratchDir {
    fn new(label: &str) -> ScratchDir {
        let dir = env::temp_dir().join(format!(
            "rune-fuzz-panic-guard-{label}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        ScratchDir(dir)
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_panic_in_the_display_checkers_is_recorded_not_unwound() {
    let _armed = fault::before_sync_idempotent_check(|| panic!("{PANIC_TEXT}"));
    let (content, actions) = session();

    let violation = driver::run(DOC_PATH, &content, &actions)
        .violation
        .expect("the forced panic must be recorded as a violation");

    assert_eq!(violation.id, "NO-PANIC");
    assert_eq!(violation.message, PANIC_TEXT);
}

#[test]
fn a_recorded_panic_names_the_file_and_line_that_raised_it() {
    let _armed = fault::before_sync_idempotent_check(|| panic!("{PANIC_TEXT}"));
    let (content, actions) = session();

    let violation = driver::run(DOC_PATH, &content, &actions)
        .violation
        .expect("the forced panic must be recorded as a violation");

    let site = violation
        .site
        .clone()
        .expect("a recorded panic must carry the site it came from");
    assert!(
        site.location.contains("tests/panic_guard.rs"),
        "the location must name the panicking file, got {:?}",
        site.location
    );
    assert!(
        !site.backtrace.is_empty(),
        "a recorded panic must carry a backtrace"
    );
    assert!(
        violation.to_string().contains(PANIC_TEXT),
        "the original assertion text must survive alongside the site"
    );
}

#[test]
fn a_panic_in_the_display_checkers_still_writes_an_artifact_bundle() {
    let _armed = fault::before_sync_idempotent_check(|| panic!("{PANIC_TEXT}"));
    let (content, actions) = session();
    let scratch = ScratchDir::new("bundle");

    let (violation, dir) = report::capture(
        &scratch.0,
        DOC_PATH,
        &content,
        &actions,
        Violation::new("UNKNOWN", "no violation was reported".to_string()),
    )
    .expect("the bundle must be written");

    assert_eq!(violation.id, "NO-PANIC");
    assert!(dir.join("script.rune").is_file());
    let text = fs::read_to_string(dir.join("report.txt")).expect("read report.txt");
    assert!(text.starts_with("invariant: NO-PANIC\n"), "{text}");
    assert!(text.contains(&format!("message: {PANIC_TEXT}\n")), "{text}");
    assert!(text.contains("\npanic location: "), "{text}");
    assert!(text.contains("tests/panic_guard.rs"), "{text}");
    assert!(text.contains("\npanic backtrace:\n"), "{text}");
}

#[test]
#[ignore = "child-mode only: invoked by the_caught_panic_still_reaches_stderr via self-exec"]
fn child_catches_a_panic() {
    let violation =
        guard::catching_panic(|| panic!("{PANIC_TEXT}")).expect_err("the closure must panic");
    assert_eq!(violation.message, PANIC_TEXT);
}

#[test]
fn the_caught_panic_still_reaches_stderr() {
    let output = Command::new(env::current_exe().expect("current test binary path"))
        .args([
            "--ignored",
            "--exact",
            "--test-threads=1",
            "--nocapture",
            "child_catches_a_panic",
        ])
        .output()
        .expect("spawn child process");

    assert!(
        output.status.success(),
        "the child must catch its own panic and pass: {output:?}"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("panicked at"),
        "the chained hook must not silence the default one, got stderr: {stderr}"
    );
    assert!(
        stderr.contains(PANIC_TEXT),
        "the default hook's own line must still carry the panic text, got stderr: {stderr}"
    );
}
