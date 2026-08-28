//! The session fuzz target. `#[ignore]` by design: a randomized soak, run
//! only via `make test-fuzz`, never inside an ordinary `make test` sweep.
//! Mirrors the `#[ignore]` convention set by the perf guards.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::Path;

use proptest::test_runner::{Config, FileFailurePersistence, TestCaseError, TestError, TestRunner};
use rune_fuzz::invariant::Violation;
use rune_fuzz::{driver, generate, report, script, wal};

#[ignore = "Randomized soak. Runs ONLY via the explicit invocation in \
            `make test-fuzz`, which sets PROPTEST_CASES/ \
            PROPTEST_RNG_SEED and scopes to `cargo nextest run -p rune-fuzz \
            --test human_session --run-ignored only -E 'test(=human_session)'` \
            rather than a bare `cargo nextest run --workspace`."]
#[test]
fn human_session() {
    match wal::sweep(Path::new("artifacts")) {
        Ok(None) => {}
        Ok(Some(dir)) => panic!(
            "a previous fuzz run died to a process-level signal mid-case; \
             its write-ahead script was promoted to {}",
            dir.display()
        ),
        Err(e) => panic!("wal::sweep failed: {e}"),
    }

    let config = Config {
        // Direct, not the default SourceParallel: SourceParallel cannot resolve
        // a path for a source file under tests/ (it looks for a sibling
        // lib.rs/main.rs and gives up). CWD for `cargo test` is the package root.
        // The path lives under the gitignored artifacts tree, not version
        // control: a random-seed fuzz failure must not dirty the working tree,
        // or the checked-in gate list stops being idempotent. Proptest creates
        // any missing intermediate directories itself before writing.
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(
            "artifacts/proptest-regressions/human_session.txt",
        ))),
        ..Config::default() // PROPTEST_CASES read here, once (LazyLock)
    };
    let mut runner = TestRunner::new(config);

    let outcome = runner.run(&generate::arb_session(), |(path, content, actions)| {
        let _wal =
            wal::arm(Path::new("artifacts"), &path, &content, &actions).unwrap_or_else(|e| {
                panic!("wal::arm failed (environment fault, not a fuzz finding): {e}")
            });
        driver::run_catching_panic(&path, &content, &actions)
            .violation
            .map_or(Ok(()), |v| Err(TestCaseError::fail(v.to_string())))
    });

    match outcome {
        Ok(()) => {}
        Err(TestError::Abort(why)) => panic!("proptest aborted: {}", why.message()),
        Err(TestError::Fail(why, (path, content, actions))) => {
            // Re-run the MINIMAL input to recover the frozen snapshot/
            // context proptest's own shrink loop discarded. The driver is
            // deterministic (tests/tripwire.rs's driver_is_deterministic),
            // so this reproduces the exact same violation.
            let (v, dir) = report::capture(
                Path::new("artifacts"),
                &path,
                &content,
                &actions,
                Violation::new("UNKNOWN", why.message().to_string()),
            )
            .expect("failed to write fuzz artifact");
            panic!(
                "{v}\nartifact: {}\nscript:\n{}\n{}",
                dir.display(),
                script::encode(&path, &content, &actions),
                runner
            );
        }
    }
}
