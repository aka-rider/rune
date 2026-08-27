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
#[should_panic(expected = "fall outside the granted claim")]
fn push_visible_asserts_a_span_only_partially_inside_its_piece() {
    // The grant is [0,4); the pushed span is [2,10) — it starts inside the
    // grant but runs well past it. Full containment (`s <= start && end <=
    // e`) is false; a `||` corruption would let either half alone pass.
    let content = "0123456789";
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

    let ll = LineLocal::clip(0, 0..10, 0..4).unwrap();
    let granted = out.claim_free(&ll);
    let span = SyntaxSpan::identical(content, crate::emit::style::text_scope(), 2..10);
    granted.push_visible(vec![span]);
}

#[test]
fn piece_is_covered_requires_full_containment_not_partial_overlap() {
    let content = "0123456789";
    let scope = crate::emit::style::text_scope();
    let spans = vec![SyntaxSpan::identical(content, scope, 0..4)];
    assert!(
        !piece_is_covered(&spans, (2, 10)),
        "a span covering only [0,4) must not count as covering the piece [2,10)"
    );
    assert!(
        piece_is_covered(&spans, (1, 3)),
        "a span [0,4) fully containing [1,3) must count as covering it"
    );
}

#[test]
fn unclaimed_on_a_never_seen_line_returns_the_real_gap_not_a_flipped_comparison() {
    // `self.merged.get(line)` is `None` for a line index past the
    // constructor's own `accounted` length — the `end > start` branch this
    // exercises. Two cases distinguish all three comparison mutants at
    // once: a genuine non-empty gap (real: the gap; `==`/`<` both wrongly
    // report empty), and a zero-length query (real: empty; `==`/`>=` both
    // wrongly report a spurious tuple).
    let mut spans: Vec<Vec<SyntaxSpan>> = vec![Vec::new()];
    let mut hidden: Accounted = vec![Vec::new()];
    let mut accounted: Accounted = vec![Vec::new()];
    let mut tables: Vec<Option<TableRowInfo>> = vec![None];
    let mut decors: Vec<Option<LineDecor>> = vec![None];
    let icons = IconSet::unicode();
    let out = EmitOut::new(
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

    assert_eq!(out.unclaimed(5, 2, 6), vec![(2, 6)]);
    assert_eq!(out.unclaimed(5, 3, 3), Vec::<(usize, usize)>::new());
}

#[test]
fn insert_merged_absorbs_a_touching_successor_during_the_forward_sweep() {
    // Seed two DISJOINT entries, then insert a range that bridges them: the
    // constructor-time merge with the predecessor empties the map before
    // the forward `while` loop ever runs, so the existing
    // `joins_overlapping_and_touching_ranges` test above never actually
    // exercises that loop's own boundary. This does: after absorbing the
    // predecessor, the loop must still walk forward and absorb the
    // touching successor into ONE final range, not stop one entry short.
    let mut map = BTreeMap::new();
    insert_merged(&mut map, 0, 4);
    insert_merged(&mut map, 10, 14);
    insert_merged(&mut map, 4, 10);
    assert_eq!(map.into_iter().collect::<Vec<_>>(), vec![(0, 14)]);
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
