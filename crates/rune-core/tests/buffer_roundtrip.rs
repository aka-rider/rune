//! Property tests mirroring the Go buffer fuzz corpus
//! (`FuzzBufferSnapshotImmutability`, `FuzzBufferBatchEquivalence`,
//! `FuzzBufferPointRoundtrip`), plus the
//! plan-required inverse/reapply round-trip through `rune_core::undo`:
//! random doc + random valid edit batch -> apply -> inverse -> byte-identical
//! original; `reapply(applied)` reproduces the edited bytes.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::type_complexity
)]

use proptest::prelude::*;
use rune_core::buffer::{Buffer, Edit};
use rune_core::coords::BufferPoint;
use rune_core::undo::{apply_inverse, reapply};

/// Port of `buffer_test.go:normalizeBounds`.
fn normalize_bounds(length: usize, start: usize, end: usize) -> (usize, usize) {
    let start = start.min(length);
    let end = end.max(start).min(length);
    (start, end)
}

proptest! {
    /// Port of `FuzzBufferSnapshotImmutability`: `Buffer::replace` never
    /// mutates the receiver, and the result's `len()` always matches its
    /// `content()` byte length.
    #[test]
    fn snapshot_immutability(
        init in ".{0,64}",
        raw_start in 0usize..80,
        raw_end in 0usize..80,
        insert in ".{0,16}",
    ) {
        let b = Buffer::new(init);
        let (start, end) = normalize_bounds(b.len(), raw_start, raw_end);

        let orig_len = b.len();
        let orig_content = b.content().to_string();

        let new_b = b.replace(start, end, &insert);

        prop_assert_eq!(b.len(), orig_len);
        prop_assert_eq!(b.content(), orig_content.as_str());
        prop_assert_eq!(new_b.len(), new_b.content().len());
    }

    /// Port of `FuzzBufferBatchEquivalence`: applying two non-overlapping
    /// edits as a descending-sorted batch produces the same content as
    /// applying them individually in descending order. Bounds are derived
    /// from four sorted raw values (`s2 <= e2 <= s1 <= e1`, all clamped to
    /// `len` — clamping a sorted sequence with a monotonic `.min` preserves
    /// order) so every generated case is valid by construction; no
    /// `prop_assume` rejection is needed.
    #[test]
    fn batch_equivalence(
        init in ".{0,64}",
        mut raw in prop::collection::vec(0usize..100, 4),
        i1 in ".{0,8}",
        i2 in ".{0,8}",
    ) {
        let len = init.len();
        raw.sort_unstable();
        let s2 = raw.first().copied().unwrap_or(0).min(len);
        let e2 = raw.get(1).copied().unwrap_or(0).min(len);
        let s1 = raw.get(2).copied().unwrap_or(0).min(len);
        let e1 = raw.get(3).copied().unwrap_or(0).min(len);

        let b = Buffer::new(init);
        let b_indiv = b.replace(s1, e1, &i1).replace(s2, e2, &i2);

        let batch_result = b.apply_edits(&[
            Edit { start: s1, end: e1, insert: i1.clone(), cursor_id: 0 },
            Edit { start: s2, end: e2, insert: i2.clone(), cursor_id: 0 },
        ]);

        if let Ok((b_batch, _)) = batch_result {
            prop_assert_eq!(b_indiv.content(), b_batch.content());
        }
    }

    /// Port of `FuzzBufferPointRoundtrip`: `line_col_to_offset` and
    /// `offset_to_line_col` are inverses for any in-range point. `raw_line`
    /// is reduced modulo the actual line count and `raw_col` modulo the
    /// actual line width (both always >= 1) so every generated point is
    /// in-range by construction; no `prop_assume` rejection is needed.
    #[test]
    fn point_roundtrip(
        init in "([^\\n]{0,12}\\n){0,5}[^\\n]{0,12}",
        raw_line in 0usize..8,
        raw_col in 0usize..20,
    ) {
        let b = Buffer::new(init);
        let line = raw_line % b.line_count();

        let start = b.line_start(line);
        let end = if line == b.line_count() - 1 {
            b.len()
        } else {
            b.line_end(line)
        };
        let col = raw_col % (end.saturating_sub(start) + 1);

        let bp = BufferPoint { line, col };
        let offset = b.line_col_to_offset(bp);
        let bp2 = b.offset_to_line_col(offset);
        prop_assert_eq!(bp, bp2);
    }

    /// Plan-required property: random doc + random valid single edit ->
    /// apply -> inverse -> byte-identical original; `reapply` reproduces
    /// the edited bytes from the original.
    #[test]
    fn edit_inverse_and_reapply_round_trip(
        init in ".{0,64}",
        raw_start in 0usize..80,
        raw_len in 0usize..20,
        insert in ".{0,16}",
    ) {
        let b = Buffer::new(init);
        let (start, end) = normalize_bounds(b.len(), raw_start, raw_start.saturating_add(raw_len));

        let edit = Edit { start, end, insert: insert.clone(), cursor_id: 0 };
        if let Ok((edited, applied)) = b.apply_edits(&[edit]) {
            let restored = apply_inverse(&edited, &applied).expect("inverse must apply cleanly");
            prop_assert_eq!(restored.content(), b.content());
            prop_assert_eq!(restored.len(), b.len());

            let redone = reapply(&b, &applied).expect("reapply must apply cleanly");
            prop_assert_eq!(redone.content(), edited.content());
        }
    }

    /// Same round-trip, but over a batch of up to three non-overlapping
    /// descending edits — exercises the multi-edit inverse/reapply paths.
    #[test]
    fn batch_inverse_and_reapply_round_trip(
        init in ".{0,96}",
        raw_s1 in 0usize..100, raw_l1 in 0usize..12, i1 in ".{0,8}",
        raw_s2 in 0usize..100, raw_l2 in 0usize..12, i2 in ".{0,8}",
        raw_s3 in 0usize..100, raw_l3 in 0usize..12, i3 in ".{0,8}",
    ) {
        let b = Buffer::new(init);
        let len = b.len();

        let (s1, e1) = normalize_bounds(len, raw_s1, raw_s1.saturating_add(raw_l1));
        let (s2, e2) = normalize_bounds(len, raw_s2, raw_s2.saturating_add(raw_l2));
        let (s3, e3) = normalize_bounds(len, raw_s3, raw_s3.saturating_add(raw_l3));

        // Force strictly descending, non-overlapping bounds with an actual
        // gap between edits (not merely touching). A zero-width edit sitting
        // exactly at another edit's boundary produces AppliedEdit.start
        // ties in the post-edit coordinate space (even when the *original*
        // starts differ), and `Reapply`/`reapply`'s ascending start-only
        // sort (`edit_primitives.go:91-93`) has no tie-break to order them
        // correctly. The real pipeline never produces touching edits in one
        // batch: `CursorSet::merge` coalesces any two cursors whose
        // selections touch into one before edits are ever generated, so
        // this is a test-construction artifact, not a reachable state —
        // require a strict gap instead of asserting an order neither Go nor
        // this port actually guarantees.
        let mut spans = [(s1, e1, i1), (s2, e2, i2), (s3, e3, i3)];
        spans.sort_by_key(|s| std::cmp::Reverse(s.0));
        prop_assume!(spans[0].0 > spans[1].1 && spans[1].0 > spans[2].1);

        let edits: Vec<Edit> = spans
            .iter()
            .map(|(s, e, ins)| Edit { start: *s, end: *e, insert: ins.clone(), cursor_id: 0 })
            .collect();

        if let Ok((edited, applied)) = b.apply_edits(&edits) {
            let restored = apply_inverse(&edited, &applied).expect("inverse must apply cleanly");
            prop_assert_eq!(restored.content(), b.content());

            let redone = reapply(&b, &applied).expect("reapply must apply cleanly");
            prop_assert_eq!(redone.content(), edited.content());
        }
    }
}

/// Review finding 6(a): the incrementally-updated line index
/// (`update_line_starts`, exercised via `apply_edits`) must always agree
/// with a from-scratch line index built by `Buffer::new` over the same
/// final content. Table cases cover the scenarios named in the finding: a
/// multi-line insert, a newline-spanning delete, and a descending
/// multi-edit batch that touches several lines in one call.
#[test]
fn incremental_line_index_matches_from_scratch_rebuild() {
    let cases: [(&str, &[(usize, usize, &str)]); 3] = [
        // Multi-line insert into a single-line buffer.
        ("hello world", &[(5, 5, "\none\ntwo\nthree")]),
        // Newline-spanning delete: removes "\nbbb\nccc", collapsing four
        // lines ("aaa","bbb","ccc","ddd") down to two ("aaa","ddd").
        ("aaa\nbbb\nccc\nddd", &[(3, 11, "")]),
        // Descending multi-edit batch touching several lines at once:
        // replace "line4" (24..29), then "line2\n" (12..18), then "line0"
        // (0..5) — total length is 29 (5 * "lineN" + 4 newlines).
        (
            "line0\nline1\nline2\nline3\nline4",
            &[(24, 29, "X"), (12, 18, "\nY\nZ"), (0, 5, "L0")],
        ),
    ];

    for (init, edits) in cases {
        let b = Buffer::new(init);
        let edit_batch: Vec<Edit> = edits
            .iter()
            .map(|&(start, end, insert)| Edit {
                start,
                end,
                insert: insert.to_string(),
                cursor_id: 0,
            })
            .collect();
        let (edited, _) = b
            .apply_edits(&edit_batch)
            .unwrap_or_else(|e| panic!("edit batch should apply for {init:?}: {e}"));
        let fresh = Buffer::new(edited.content());

        assert_eq!(
            edited.line_count(),
            fresh.line_count(),
            "line_count mismatch for case {init:?} -> {:?}",
            edited.content()
        );
        for i in 0..fresh.line_count() {
            assert_eq!(
                edited.line_start(i),
                fresh.line_start(i),
                "line_start({i}) mismatch for case {init:?} -> {:?}",
                edited.content()
            );
            assert_eq!(
                edited.line(i),
                fresh.line(i),
                "line({i}) mismatch for case {init:?} -> {:?}",
                edited.content()
            );
        }
    }
}

/// Review finding 6(b) / regression for finding 1:
/// `Buffer::default()` used to derive `line_starts: vec![]` via
/// `#[derive(Default)]`, silently breaking the `line_starts[0] == 0`
/// invariant on the very first edit. `Buffer::default()` must behave
/// identically to `Buffer::new("")` for every line-index-observing method.
#[test]
fn default_buffer_then_insert_has_correct_line_index() {
    let b = Buffer::default().insert(0, "hello\nworld");

    assert_eq!(b.line_count(), 2);
    assert_eq!(b.line(0), "hello");
    assert_eq!(b.line(1), "world");
    assert_eq!(b.offset_to_line_col(8), BufferPoint { line: 1, col: 2 });
    assert_eq!(b.line_col_to_offset(BufferPoint { line: 1, col: 2 }), 8);
}

/// `Buffer::from_bytes` refuses invalid UTF-8. Moved here from the vfs
/// round-trip suite (WP1, rune-vfs split) — this exercises `Buffer`, not
/// the vfs, so it belongs with the rest of the buffer tests.
#[test]
fn buffer_refuses_invalid_utf8() {
    let result = Buffer::from_bytes(vec![0xff, 0xfe]);
    assert!(result.is_err(), "invalid UTF-8 should be refused");
}
