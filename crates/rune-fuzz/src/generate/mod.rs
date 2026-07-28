//! The session generator. WP3 shipped a UNIFORM `prop_oneof!` over the eight
//! `Action` variants. WP6 replaces it with the user-approved "normal human
//! session" weighted table (a `prop_oneof!` over 11 clusters instead of bare
//! actions) without changing `arb_session`'s signature — every caller (the
//! fuzz target, the tripwire round-trip test, the replay test) keeps
//! working unmodified.
//!
//! Each cluster is a `Strategy<Value = Vec<Action>>`; a whole session is
//! `vec(cluster, 1..=40).prop_map(|v| v.concat())`, truncated to 120 actions
//! and paired with a seed from `CONTENT_SEEDS` (plan Assumption A3). The
//! static palette data lives in `palette.rs` and the `cluster_*` strategies
//! in `cluster.rs` (§1.6 budget) — this file is left with just the public
//! entry point tying the two together.

mod cluster;
mod palette;

use proptest::prelude::*;
use proptest::sample::select;

use crate::action::Action;

pub use palette::TYPE_PALETTE;

use cluster::arb_cluster;
use palette::CONTENT_SEEDS;

/// One whole fuzz case: the seed content plus a weighted "normal human
/// session" of 1..=40 clusters, concatenated and capped at 120 actions
/// (plan Assumption A3, mirroring Go's `maxHumanEvents = 160`).
pub fn arb_session() -> impl Strategy<Value = (String, Vec<Action>)> {
    (
        select(CONTENT_SEEDS).prop_map(str::to_string),
        proptest::collection::vec(arb_cluster(), 1..=40).prop_map(|clusters| {
            let mut actions = clusters.concat();
            actions.truncate(120);
            actions
        }),
    )
}
