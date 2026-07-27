//! WP5.S2 + WP5.S7, kept in their own file rather than growing
//! `tests/tripwire.rs` (already 426 lines before this work; §1.6 caps a
//! file at 500 LoC — "decompose any file past 500 LoC"). Both non-`#[
//! ignore]`, so `make test` runs them.
//!
//! - `codec_round_trips_every_generated_session` (WP5.S2) — `script::
//!   encode`/`decode` round-trip over every session `generate::
//!   arb_session()` can produce.
//! - `report_writer_produces_a_replayable_bundle` (WP5.S7) — `report::
//!   write`'s bundle is itself replayable: both files exist, the written
//!   `script.rune` decodes back to the original `(content, actions)`, and
//!   replaying it reproduces the frozen `final_content`.
//!
//! S7 sources its script from `repros/tripwire-clean.rune` (WP5.S5)
//! instead of duplicating `tests/tripwire.rs`'s `tripwire_script()`/
//! `FIXTURE` — one source for that script, decoded through the same
//! `script::decode` the rest of this crate uses.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::fs;

use proptest::prelude::*;

use rune_fuzz::invariant::Violation;
use rune_fuzz::{driver, generate, report, script};

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// WP5.S2 — `decode(&encode(&c, &a)) == Ok((c, a))` over every session
    /// the generator can produce. Does not run the driver, so it is safe
    /// under `strict-invariants`.
    #[test]
    fn codec_round_trips_every_generated_session((content, actions) in generate::arb_session()) {
        let encoded = script::encode(&content, &actions);
        prop_assert_eq!(script::decode(&encoded), Ok((content, actions)));
    }
}

/// WP5.S7 — a permanent, self-cleaning regression that the report bundle
/// `report::write` produces is replayable, not just "written". Builds a
/// synthetic `Violation` (this test does not sabotage any production code
/// path to manufacture a real one) over the WP4 tripwire script, writes it
/// into a scratch directory under `std::env::temp_dir()` that this test
/// creates and removes itself, then asserts:
/// (a) both `report.txt` and `script.rune` exist;
/// (b) `script::decode` of the written `script.rune` round-trips to the
///     original `(content, actions)`;
/// (c) `driver::run` on the decoded pair reproduces the same
///     `final_content` the original run produced.
#[test]
fn report_writer_produces_a_replayable_bundle() {
    let script_text = include_str!("../repros/tripwire-clean.rune");
    let (content, actions) = script::decode(script_text)
        .unwrap_or_else(|e| panic!("repros/tripwire-clean.rune failed to decode: {e}"));

    let result = driver::run(&content, &actions);
    let violation = Violation {
        id: "TRIPWIRE-PROBE",
        message: "synthetic violation for report::write's own regression test \
                  (report_writer_produces_a_replayable_bundle); not a real catch"
            .to_string(),
    };

    let scratch = std::env::temp_dir().join(format!(
        "rune-fuzz-codec-and-report-test-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&scratch);

    let dir = report::write(&scratch, &violation, &content, &actions, &result)
        .unwrap_or_else(|e| panic!("report::write failed: {e}"));

    assert!(
        dir.join("script.rune").is_file(),
        "script.rune missing in {}",
        dir.display()
    );
    assert!(
        dir.join("report.txt").is_file(),
        "report.txt missing in {}",
        dir.display()
    );

    let written = fs::read_to_string(dir.join("script.rune"))
        .unwrap_or_else(|e| panic!("failed to read written script.rune: {e}"));
    let (decoded_content, decoded_actions) = script::decode(&written)
        .unwrap_or_else(|e| panic!("written script.rune failed to decode: {e}"));
    assert_eq!(
        decoded_content, content,
        "script.rune content did not round-trip"
    );
    assert_eq!(
        decoded_actions, actions,
        "script.rune actions did not round-trip"
    );

    let replayed = driver::run(&decoded_content, &decoded_actions);
    assert_eq!(
        replayed.final_content, result.final_content,
        "replaying the decoded bundle did not reproduce the original run's final_content"
    );

    let _ = fs::remove_dir_all(&scratch);
}
