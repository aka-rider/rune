//! The session generator. WP3 shipped a UNIFORM `prop_oneof!` over every
//! `Action` variant that existed then (a stale "eight" here used to claim a
//! fixed count for a growing enum — CODE-REVIEW.md rune-fuzz finding 16;
//! never restate the count, since `Action` has grown every time a later
//! work package added a synthesized-reply shape). WP6 replaces it with the
//! user-approved "normal human session" weighted table (a `prop_oneof!`
//! over the `cluster_*` strategies in `cluster.rs` instead of bare
//! actions) without changing `arb_session`'s signature — every caller (the
//! fuzz target, the tripwire round-trip test, the replay test) keeps
//! working unmodified.
//!
//! Each cluster is a `Strategy<Value = Vec<Action>>`; a whole session is
//! `vec(cluster, 1..=40).prop_map(|v| v.concat())`, truncated to 120 actions
//! and paired with a `(path, content)` seed from `SEEDS` (plan Assumption
//! A3, extended by WP7.S3 to include non-markdown paths). The static
//! palette data lives in `palette.rs` and the `cluster_*` strategies in
//! `cluster.rs` (§1.6 budget) — this file is left with just the public
//! entry point tying the two together.

mod cluster;
mod palette;

use proptest::prelude::*;
use proptest::sample::select;

use crate::action::Action;

pub use palette::TYPE_PALETTE;

use cluster::arb_cluster;
use palette::SEEDS;

/// One whole fuzz case: `(path, content, actions)` — the seed path and
/// content (plan WP7.S2/S3) plus a weighted "normal human session" of
/// 1..=40 clusters, concatenated and capped at 120 actions (plan Assumption
/// A3, mirroring Go's `maxHumanEvents = 160`).
pub fn arb_session() -> impl Strategy<Value = (String, String, Vec<Action>)> {
    (
        select(SEEDS).prop_map(|(path, content)| (path.to_string(), content.to_string())),
        proptest::collection::vec(arb_cluster(), 1..=40).prop_map(|clusters| {
            let mut actions = clusters.concat();
            actions.truncate(120);
            actions
        }),
    )
        .prop_map(|((path, content), actions)| (path, content, actions))
}
