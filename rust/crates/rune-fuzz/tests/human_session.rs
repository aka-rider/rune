//! The session fuzz target. `#[ignore]` by design: a randomized soak, run
//! only via `make test-fuzz` (rust/Makefile, owned by a parallel work
//! package), never inside an ordinary parallel `cargo test`. Mirrors the
//! `#[ignore]` convention set by `crates/rune-md/tests/perf_guard.rs`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use proptest::test_runner::{Config, FileFailurePersistence, TestCaseError, TestError, TestRunner};
use rune_fuzz::{driver, generate};

#[ignore = "Randomized soak. Runs ONLY via the explicit invocation in \
            rust/Makefile (`make test-fuzz`), which sets PROPTEST_CASES and \
            scopes to `-p rune-fuzz --test human_session` so rune-md's \
            strict-invariants feature stays off (see crates/rune-md/TODO.md)."]
#[test]
fn human_session() {
    let config = Config {
        // Direct, not the default SourceParallel: SourceParallel cannot resolve
        // a path for a source file under tests/ (it looks for a sibling
        // lib.rs/main.rs and gives up). CWD for `cargo test` is the package root.
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(
            "proptest-regressions/human_session.txt",
        ))),
        ..Config::default() // PROPTEST_CASES read here, once (LazyLock)
    };
    let mut runner = TestRunner::new(config);

    let outcome = runner.run(
        &generate::arb_session(),
        |(content, actions)| match driver::run(&content, &actions).violation {
            None => Ok(()),
            Some(v) => Err(TestCaseError::fail(format!("{}: {}", v.id, v.message))),
        },
    );

    match outcome {
        Ok(()) => {}
        Err(TestError::Abort(why)) => panic!("proptest aborted: {}", why.message()),
        Err(TestError::Fail(why, minimal)) => {
            // A later work package (WP5) replaces this arm with a written
            // report + minimal `.rune` script. For WP3, print enough to
            // diagnose a catch by hand: the invariant id/message and the
            // shrunk minimal input.
            panic!("{}\nminimal: {:#?}\n{}", why.message(), minimal, runner);
        }
    }
}
