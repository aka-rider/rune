#![allow(clippy::indexing_slicing, clippy::unwrap_used, clippy::expect_used)]
use super::*;

fn merged_of(pieces: &[(usize, usize)]) -> BTreeMap<usize, usize> {
    let mut map = BTreeMap::new();
    for &(s, e) in pieces {
        insert_merged(&mut map, s, e);
    }
    map
}

#[test]
fn unclaimed_subranges_skips_already_claimed_bytes() {
    let pieces = unclaimed_subranges_in_merged(0, 8, &merged_of(&[(2, 6)]));
    assert_eq!(pieces, vec![(0, 2), (6, 8)]);

    assert_eq!(
        unclaimed_subranges_in_merged(2, 6, &merged_of(&[(0, 8)])),
        Vec::<(usize, usize)>::new()
    );

    assert_eq!(
        unclaimed_subranges_in_merged(0, 4, &merged_of(&[(10, 12)])),
        vec![(0, 4)]
    );

    assert_eq!(
        unclaimed_subranges_in_merged(0, 10, &merged_of(&[(6, 8), (1, 3), (3, 4)])),
        vec![(0, 1), (4, 6), (8, 10)]
    );
}

#[test]
fn insert_merged_joins_overlapping_and_touching_ranges() {
    let mut map = BTreeMap::new();
    insert_merged(&mut map, 0, 4);
    insert_merged(&mut map, 4, 9);
    assert_eq!(map.into_iter().collect::<Vec<_>>(), vec![(0, 9)]);

    let mut map = BTreeMap::new();
    insert_merged(&mut map, 0, 5);
    insert_merged(&mut map, 3, 8);
    assert_eq!(map.into_iter().collect::<Vec<_>>(), vec![(0, 8)]);
}

#[test]
fn insert_merged_claim_spanning_whole_line_leaves_nothing_unclaimed() {
    let map = merged_of(&[(0, 20)]);
    assert_eq!(
        unclaimed_subranges_in_merged(0, 20, &map),
        Vec::<(usize, usize)>::new()
    );
}

#[test]
fn duplicate_claim_of_the_same_range_yields_nothing_the_second_time() {
    let map = merged_of(&[(2, 6), (2, 6)]);
    assert_eq!(map.into_iter().collect::<Vec<_>>(), vec![(2, 6)]);
    assert_eq!(
        unclaimed_subranges_in_merged(2, 6, &merged_of(&[(2, 6)])),
        Vec::<(usize, usize)>::new()
    );
}

#[test]
fn interleaved_unclaimed_queries_between_claims() {
    let mut map = BTreeMap::new();
    assert_eq!(unclaimed_subranges_in_merged(0, 12, &map), vec![(0, 12)]);

    insert_merged(&mut map, 2, 4);
    assert_eq!(
        unclaimed_subranges_in_merged(0, 12, &map),
        vec![(0, 2), (4, 12)]
    );

    insert_merged(&mut map, 8, 10);
    assert_eq!(
        unclaimed_subranges_in_merged(0, 12, &map),
        vec![(0, 2), (4, 8), (10, 12)]
    );
}

#[test]
fn dropped_claim_leaves_accounted_unchanged() {
    let mut spans: Vec<Vec<SyntaxSpan>> = vec![Vec::new()];
    let mut hidden: Accounted = vec![Vec::new()];
    let mut accounted: Accounted = vec![Vec::new()];
    let mut tables: Vec<Option<TableRowInfo>> = vec![None];
    let mut decors: Vec<Option<LineDecor>> = vec![None];
    let icons = IconSet::unicode();
    let mut out = EmitOut::new(
        Sinks {
            spans: &mut spans,
            hidden: &mut hidden,
            accounted: &mut accounted,
        },
        &mut tables,
        80,
        &icons,
        &mut decors,
    );

    let ll = LineLocal::clip(0, 0..4, 0..4).unwrap();
    let granted = out.claim_free(&ll);
    drop(granted);

    assert_eq!(accounted[0], Vec::<(usize, usize)>::new());
}

#[test]
fn claim_whole_grants_an_empty_range_even_when_the_line_is_fully_claimed() {
    let mut spans: Vec<Vec<SyntaxSpan>> = vec![Vec::new()];
    let mut hidden: Accounted = vec![Vec::new()];
    let mut accounted: Accounted = vec![vec![(0, 8)]];
    let mut tables: Vec<Option<TableRowInfo>> = vec![None];
    let mut decors: Vec<Option<LineDecor>> = vec![None];
    let icons = IconSet::unicode();
    let mut out = EmitOut::new(
        Sinks {
            spans: &mut spans,
            hidden: &mut hidden,
            accounted: &mut accounted,
        },
        &mut tables,
        80,
        &icons,
        &mut decors,
    );

    let ll = LineLocal::clip(0, 0..8, 4..4).unwrap();
    let result = out.claim_whole(&ll);

    assert!(result.is_ok());
}

#[test]
#[should_panic(expected = "is not entirely free")]
fn claim_whole_asserts_on_a_refused_overlap() {
    let mut spans: Vec<Vec<SyntaxSpan>> = vec![Vec::new()];
    let mut hidden: Accounted = vec![Vec::new()];
    let mut accounted: Accounted = vec![vec![(2, 4)]];
    let mut tables: Vec<Option<TableRowInfo>> = vec![None];
    let mut decors: Vec<Option<LineDecor>> = vec![None];
    let icons = IconSet::unicode();
    let mut out = EmitOut::new(
        Sinks {
            spans: &mut spans,
            hidden: &mut hidden,
            accounted: &mut accounted,
        },
        &mut tables,
        80,
        &icons,
        &mut decors,
    );

    let ll = LineLocal::clip(0, 0..8, 0..8).unwrap();
    let _ = out.claim_whole(&ll);
}

#[test]
fn refused_whole_claim_leaves_accounted_untouched() {
    let mut spans: Vec<Vec<SyntaxSpan>> = vec![Vec::new()];
    let mut hidden: Accounted = vec![Vec::new()];
    let mut accounted: Accounted = vec![vec![(2, 4)]];
    let mut tables: Vec<Option<TableRowInfo>> = vec![None];
    let mut decors: Vec<Option<LineDecor>> = vec![None];
    let icons = IconSet::unicode();
    let mut out = EmitOut::new(
        Sinks {
            spans: &mut spans,
            hidden: &mut hidden,
            accounted: &mut accounted,
        },
        &mut tables,
        80,
        &icons,
        &mut decors,
    );

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let ll = LineLocal::clip(0, 0..8, 0..8).unwrap();
        out.claim_whole(&ll).is_err()
    }));

    assert!(result.is_err());
    let _ = out;
    assert_eq!(accounted[0], vec![(2, 4)]);
}

#[test]
#[should_panic(expected = "not fully covered")]
fn push_visible_catches_a_dropped_span_leaving_a_granted_piece_uncovered() {
    let mut spans: Vec<Vec<SyntaxSpan>> = vec![Vec::new()];
    let mut hidden: Accounted = vec![Vec::new()];
    let mut accounted: Accounted = vec![Vec::new()];
    let mut tables: Vec<Option<TableRowInfo>> = vec![None];
    let mut decors: Vec<Option<LineDecor>> = vec![None];
    let icons = IconSet::unicode();
    let mut out = EmitOut::new(
        Sinks {
            spans: &mut spans,
            hidden: &mut hidden,
            accounted: &mut accounted,
        },
        &mut tables,
        80,
        &icons,
        &mut decors,
    );

    let ll = LineLocal::clip(0, 0..4, 0..4).unwrap();
    let granted = out.claim_free(&ll);
    granted.push_visible(Vec::new());
}
