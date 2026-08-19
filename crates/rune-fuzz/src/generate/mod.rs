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
//! `cluster.rs` (500-line budget) — this file is left with just the
//! public entry point tying the two together.

mod arb;
mod cluster;
mod palette;

use proptest::prelude::*;
use proptest::sample::select;

use crate::action::Action;

pub use palette::TYPE_PALETTE;

use cluster::arb_cluster;
use palette::SEEDS;

pub fn arb_session() -> impl Strategy<Value = (String, String, Vec<Action>)> {
    (
        select(SEEDS).prop_map(|(path, content)| (path.to_string(), content.to_string())),
        proptest::collection::vec(arb_cluster(), 1..=40).prop_map(|clusters| clusters.concat()),
        proptest::option::weighted(0.2, proptest::num::u8::ANY),
    )
        .prop_map(|((path, content), mut actions, diff_left)| {
            if let Some(seed_index) = diff_left {
                actions.insert(0, Action::InstallDiffLeft { seed_index });
            }
            actions.truncate(120);
            (path, content, actions)
        })
}

pub fn diff_left_content(seed_index: u8) -> &'static str {
    let Some(len) = std::num::NonZeroUsize::new(SEEDS.len()) else {
        return "";
    };
    let index = usize::from(seed_index) % len;
    SEEDS.get(index).map_or("", |(_, content)| content)
}
