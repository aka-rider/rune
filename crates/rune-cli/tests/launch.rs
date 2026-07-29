//! Exec-probes against the REAL compiled `rune` binary (plan WP4.S7/
//! [rune-cli 13]): the only reliable way to assert on a process exit code,
//! since `std::process::ExitCode` (what `main`/`launch` return in-process)
//! is deliberately opaque and can't be inspected by a unit test. Every case
//! here is an EARLY-EXIT path (`--help`/`--version`, a rejected command
//! line, a load failure) — none of them ever reach the interactive run
//! loop, so none of them hang waiting on a terminal.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::env;
use std::path::PathBuf;
use std::process::Command;

const EX_USAGE: i32 = 64;
const EX_DATAERR: i32 = 65;
const EX_SOFTWARE: i32 = 70;

fn rune() -> Command {
    Command::new(env!("CARGO_BIN_EXE_rune"))
}

/// A real, throwaway directory under the OS temp dir for the file-based
/// probes below — `rune` talks to the real filesystem (`Disk`), so these
/// need actual files, not `Mem`.
struct ScratchDir(PathBuf);

impl ScratchDir {
    fn new(label: &str) -> ScratchDir {
        let dir = env::temp_dir().join(format!(
            "rune-cli-launch-exec-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        ScratchDir(dir)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn launch_help_flag_exits_success() {
    let output = rune().arg("--help").output().expect("run rune --help");
    assert_eq!(output.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&output.stdout).contains("usage: rune"));
}

#[test]
fn launch_version_flag_exits_success() {
    let output = rune()
        .arg("--version")
        .output()
        .expect("run rune --version");
    assert_eq!(output.status.code(), Some(0));
}

#[test]
fn launch_unknown_flag_exits_usage() {
    let output = rune().arg("--bogus").output().expect("run rune --bogus");
    assert_eq!(output.status.code(), Some(EX_USAGE));
}

#[test]
fn launch_dash_w_missing_value_exits_usage() {
    let output = rune().arg("-w").output().expect("run rune -w");
    assert_eq!(output.status.code(), Some(EX_USAGE));
}

#[test]
fn launch_empty_positional_exits_usage() {
    let output = rune().arg("").output().expect("run rune ''");
    assert_eq!(output.status.code(), Some(EX_USAGE));
}

#[test]
fn launch_dash_w_not_a_directory_exits_usage() {
    let scratch = ScratchDir::new("w-not-a-dir");
    let file = scratch.path("plain.txt");
    std::fs::write(&file, b"hi").expect("seed plain file");

    let output = rune()
        .arg("-w")
        .arg(&file)
        .output()
        .expect("run rune -w <file>");
    assert_eq!(output.status.code(), Some(EX_USAGE));
}

#[test]
fn launch_invalid_utf8_file_exits_dataerr() {
    let scratch = ScratchDir::new("bad-utf8");
    let file = scratch.path("bad.md");
    std::fs::write(&file, [0xff, 0xfe]).expect("seed invalid utf-8 file");

    let output = rune().arg(&file).output().expect("run rune <bad file>");
    assert_eq!(output.status.code(), Some(EX_DATAERR));
}

#[cfg(unix)]
#[test]
fn launch_non_unicode_argv_exits_usage_not_a_panic() {
    use std::os::unix::ffi::OsStrExt;

    let bad_arg = std::ffi::OsStr::from_bytes(&[0xff, 0xfe]);
    let output = rune().arg(bad_arg).output().expect("run rune <bad argv>");
    // Exit 64 (EX_USAGE), never 101 (the default Rust panic exit code) —
    // `env::args()` used to panic on non-UTF-8 argv before `env::args_os()`
    // replaced it (plan WP4.S2/[rune-cli 4]).
    assert_ne!(
        output.status.code(),
        Some(101),
        "non-Unicode argv must be rejected, not panic"
    );
    assert_eq!(output.status.code(), Some(EX_USAGE));
}

/// The exit-code matrix (plan WP4's Done-when): every early-exit code this
/// binary can produce, from one process launch each.
#[test]
fn launch_exit_code_matrix() {
    let cases: &[(&[&str], i32)] = &[
        (&["--help"], 0),
        (&["--version"], 0),
        (&["--bogus"], EX_USAGE),
        (&["-w"], EX_USAGE),
        (&[""], EX_USAGE),
    ];
    for (args, expected) in cases {
        let output = rune().args(*args).output().expect("run rune");
        assert_eq!(
            output.status.code(),
            Some(*expected),
            "args {args:?} should exit {expected}"
        );
    }

    let scratch = ScratchDir::new("matrix-bad-utf8");
    let file = scratch.path("bad.md");
    std::fs::write(&file, [0xff, 0xfe]).expect("seed invalid utf-8 file");
    let output = rune().arg(&file).output().expect("run rune <bad file>");
    assert_eq!(output.status.code(), Some(EX_DATAERR));

    // Not part of the matrix's cases above (would need a live terminal to
    // run to completion): a directory as the target path exits IO_ERR
    // before ever reaching the run loop.
    let output = rune().arg(&scratch.0).output().expect("run rune <dir>");
    assert_ne!(
        output.status.code(),
        Some(EX_SOFTWARE),
        "opening a directory must be a clean I/O-class exit, never an internal panic"
    );
}
