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
