//! WP5.S6 — permanent replay regression. NOT `#[ignore]`d: runs on every
//! `cargo test --workspace` / `make test`. Reads every `*.rune` script
//! directly under `crates/rune-fuzz/repros/` (skipping the
//! `strict-known/` subdirectory, which `rune-md`'s known-open
//! strict-invariants panics belong in per plan Gotcha G1, and this
//! directory's own `README.md`), `script::decode`s each one, drives it
//! through `driver::run`, and asserts no invariant fires.
//!
//! Deliberately non-vacuous (plan Gotcha G2's class of failure mode: a
//! filter or glob that matches nothing still prints `test result: ok` and
//! exits 0). This file instead:
//! - fails outright if `repros/` does not exist;
//! - fails outright if the glob reads zero scripts;
//! - `println!`s the file name of every script it actually runs, so
//!   `--nocapture` output is direct proof of what ran, not an inference
//!   from the test passing.
//!
//! Set `RUNE_FUZZ_REPLAY` to replay specific scripts instead of scanning
//! `repros/` — for example a fresh `crates/rune-fuzz/artifacts/<id>-<hash>/script.rune`
//! under triage. Paths are separated by `:` (a colon cannot occur in a
//! macOS/Linux path) and may be absolute or relative to the package root.
//! The same non-vacuity guarantee applies: an override that names zero
//! readable paths fails the test.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::fs;
use std::path::{Path, PathBuf};

use rune_fuzz::{driver, script};

/// Directly under `repros/`: package-root-relative, since `cargo test`
/// runs integration tests with CWD = the package root (plan G11's same
/// observation, reused here).
const REPROS_DIR: &str = "repros";

const REPLAY_OVERRIDE_VAR: &str = "RUNE_FUZZ_REPLAY";
const REPLAY_OVERRIDE_SEPARATOR: char = ':';

fn replay_override() -> Option<Vec<PathBuf>> {
    let raw = std::env::var(REPLAY_OVERRIDE_VAR).ok()?;
    Some(
        raw.split(REPLAY_OVERRIDE_SEPARATOR)
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .collect(),
    )
}

/// Collects every `*.rune` path directly under `REPROS_DIR`, skipping the
/// `strict-known/` subdirectory (not a file, so a plain `is_file` +
/// extension filter already excludes it, but the name is called out here
/// for anyone reading this next to the README) and non-`.rune` files such
/// as `README.md`.
fn collect_repro_scripts(dir: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", dir.display()))
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|ext| ext == "rune"))
        .collect();
    paths.sort();
    paths
}

#[test]
fn every_checked_in_repro_replays_clean() {
    let scripts = if let Some(paths) = replay_override() {
        assert!(
            !paths.is_empty(),
            "{REPLAY_OVERRIDE_VAR} was set but named zero paths — an empty \
             override must never silently pass (plan Gotcha G2)",
        );
        paths
    } else {
        let dir = Path::new(REPROS_DIR);
        assert!(
            dir.is_dir(),
            "{} must exist and be a directory (checked-in fuzz repros) — \
             a missing directory here would make this test vacuously pass \
             over zero scripts (plan Gotcha G2)",
            dir.display()
        );

        let scripts = collect_repro_scripts(dir);
        assert!(
            !scripts.is_empty(),
            "found zero `*.rune` scripts directly under {} — an empty glob must \
             never silently pass (plan Gotcha G2); repros/tripwire-clean.rune \
             is checked in for exactly this reason",
            dir.display()
        );
        scripts
    };

    for path in &scripts {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("<unnamed>");
        println!("replaying {name}");

        let text = fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
        let (doc_path, content, actions) = script::decode(&text)
            .unwrap_or_else(|e| panic!("{} failed to decode: {e}", path.display()));

        let result = driver::run_catching_panic(&doc_path, &content, &actions);
        assert!(
            result.violation.is_none(),
            "{}: {}",
            name,
            result
                .violation
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_default()
        );
    }
}
