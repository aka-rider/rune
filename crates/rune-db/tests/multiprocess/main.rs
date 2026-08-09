//! Multiprocess integration tests (plan WP6.S4, R4's resolution: "the
//! multiprocess tests in WP6 must spawn real child processes" — same-process
//! threads share SQLite's in-process lock table and never exercise the real
//! cross-process locking this crate depends on).
//!
//! Split across sibling files (line-budget rule) but compiled as the ONE
//! `multiprocess` test binary — `cargo test`'s directory convention (a
//! `main.rs` under a `tests/<name>/` directory is still exactly one
//! integration-test target named `<name>`) — never several. Splitting into
//! SEPARATE test binaries instead would have let cargo schedule them in
//! parallel where they used to run sequentially inside one process, and
//! every scenario here rendezvouses real child processes through
//! filesystem marker files under its own freshly minted temp directory;
//! nothing about that handshake is safe to race against another scenario's
//! children sharing the same test run. Staying one binary preserves the
//! exact sequential scheduling this file always had.
//!
//! # The re-exec-self pattern
//!
//! Each scenario test spawns `std::env::current_exe()` (THIS test binary) as
//! a child process with `--exact helper_entrypoint --nocapture` and a
//! `RUNE_DB_HELPER=<role>` environment variable. `cargo test`'s own harness
//! then runs ONLY [`helper_entrypoint`] in the child (no custom `main`/
//! `#[ctor]` needed); that test reads `RUNE_DB_HELPER`, dispatches to the
//! matching role in `helper`, and the role function itself calls
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

mod fail_fast;
mod helper;
mod scenarios;
mod support;

const ROLE_ENV: &str = "RUNE_DB_HELPER";

/// When `RUNE_DB_HELPER` is unset (every normal `cargo test` run), a no-op
/// passing test. When a scenario spawns this same binary with `--exact
/// helper_entrypoint --nocapture` and `RUNE_DB_HELPER` set, runs the
/// requested role and exits the process itself.
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
        "gc_editor" => helper::gc_editor(),
        "gc_sweeper" => helper::gc_sweeper(),
        "edit_and_die" => helper::edit_and_die(),
        "reload_diverged" => helper::reload_diverged(),
        other => {
            eprintln!("multiprocess helper: unknown role {other}");
            std::process::exit(2);
        }
    }
}
