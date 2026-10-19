#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::*;
use crate::history::LineageStep;

fn lineage(base: i64, done: &[i64], undone: &[i64]) -> Lineage {
    let steps = |seqs: &[i64]| {
        seqs.iter()
            .map(|&seq| LineageStep {
                seq: Seq(seq),
                ..LineageStep::default()
            })
            .collect()
    };
    Lineage {
        base_seq: Seq(base),
        done: steps(done),
        undone: steps(undone),
    }
}

fn state_for(origin: &TokenOrigin) -> DocUndoState {
    let mut states = HashMap::new();
    ensure_token_state(&mut states, BindingToken::next(), origin);
    states.into_values().next().expect("seeded state")
}

#[test]
fn an_origin_at_a_seq_resolves_only_local_position_zero() {
    let mut state = state_for(&TokenOrigin::at(Seq(7)));

    assert_eq!(state.resolve(0), Some(Seq(7)));
    assert_eq!(state.resolve(1), None);
}

#[test]
fn an_origin_from_a_lineage_resolves_every_stored_step_in_both_directions() {
    let mut state = state_for(&TokenOrigin::from_lineage(&lineage(10, &[12, 15], &[18])));

    assert_eq!(state.resolve(0), Some(Seq(10)));
    assert_eq!(state.resolve(1), Some(Seq(12)));
    assert_eq!(state.resolve(2), Some(Seq(15)));
    assert_eq!(state.resolve(3), Some(Seq(18)));
    assert_eq!(state.resolve(4), None);
}

#[test]
fn an_append_after_a_lineage_origin_replaces_the_redo_tail() {
    let mut state = state_for(&TokenOrigin::from_lineage(&lineage(10, &[12, 15], &[18])));

    state.push_seq(DocId(1), Seq(20));

    assert_eq!(state.resolve(3), Some(Seq(20)));
    assert_eq!(state.resolve(4), None);
}
