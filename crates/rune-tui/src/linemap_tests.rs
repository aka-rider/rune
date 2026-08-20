use super::*;

fn recon(r: Range<usize>) -> Range<ReconOffset> {
    ReconOffset(r.start)..ReconOffset(r.end)
}

fn buf(r: Range<usize>) -> Range<BufOffset> {
    BufOffset(r.start)..BufOffset(r.end)
}

/// A top-level fence: consecutive lines are truly adjacent in the buffer
/// (the gap is exactly one real `'\n'`), so the reconstructed text is
/// byte-identical to the buffer slice.
fn contiguous() -> (&'static str, LineMap) {
    let content = "let a = 1;\nlet b = 2;";
    (content, LineMap::new(content, vec![0..10, 11..21]))
}

/// A blockquoted fence: line 1 starts after `"> "`, two bytes the
/// reconstructed text never sees.
fn nested() -> (&'static str, LineMap) {
    let content = "let a = 1;\n> let b = 2;";
    (content, LineMap::new(content, vec![0..10, 13..23]))
}

/// A one-line fence against `content`. Written as a call rather than
/// `vec![line]` because a single range literal inside a `vec!` is a
/// hard clippy error.
fn one_line(content: &str, line: Range<usize>) -> LineMap {
    LineMap::new(content, vec![line])
}

#[test]
fn reconstruct_drops_a_container_prefix_but_keeps_a_contiguous_slice_verbatim() {
    let (content, map) = contiguous();
    assert_eq!(map.reconstruct(content).unwrap(), content);

    let (content, map) = nested();
    assert_eq!(map.reconstruct(content).unwrap(), "let a = 1;\nlet b = 2;");
}

#[test]
fn reconstruct_reports_none_for_a_line_off_the_live_buffer() {
    let map = LineMap::new("short", vec![0..10, 40..50]);
    assert!(map.reconstruct("short").is_none());
}

#[test]
fn to_buffer_is_identity_for_buffer_contiguous_lines() {
    let (content, map) = contiguous();
    assert_eq!(&content[11..21], "let b = 2;");

    let mapped = map.to_buffer(recon(11..15));
    assert_eq!(mapped.len(), 1, "a single-line range must map to one piece");
    assert_eq!(mapped[0].line(), 1);
    assert_eq!(mapped[0].range(), 11..15);
    assert_eq!(&content[mapped[0].range()], "let ");
}

#[test]
fn to_buffer_skips_the_gap_between_nested_lines() {
    let (content, map) = nested();
    assert_eq!(&content[13..23], "let b = 2;");

    // Reconstructed text is "let a = 1;\nlet b = 2;", so offsets 11..14
    // are line 1's own "let" and must land on line 1's real buffer bytes,
    // never inside the "> " gap.
    let mapped = map.to_buffer(recon(11..14));
    assert_eq!(mapped.len(), 1);
    assert_eq!(&content[mapped[0].range()], "let");
}

#[test]
fn to_buffer_end_boundary_never_lands_in_the_prefix() {
    let content = "ab\n> cd";
    let map = LineMap::new(content, vec![0..2, 5..7]);

    // Reconstructed text is "ab\ncd". A range covering "ab" plus its
    // joining '\n' must map to the real newline's own end in the buffer,
    // never into the "> " gap that follows it.
    let mapped = map.to_buffer(recon(0..3));
    assert_eq!(mapped.len(), 1);
    assert_eq!(mapped[0].range(), 0..3);
    assert_eq!(&content[mapped[0].range()], "ab\n");
}

/// The load-bearing regression case: a token starting on one physical
/// line and ending on the next must come back as TWO pieces, each
/// entirely inside its own line's bounds — never a single contiguous
/// range that would swallow the container prefix sitting in the gap
/// between them.
#[test]
fn to_buffer_splits_a_cross_line_range_at_the_line_boundary_instead_of_spanning_the_gap() {
    let content = "let a = 1;\n> let b = 2;";
    let map = LineMap::new(content, vec![0..10, 13..23]);

    // Reconstructed text is "let a = 1;\nlet b = 2;". 8..14 covers
    // "1;\nlet" — the tail of line 0, the joining newline, and the head
    // of line 1.
    let mapped = map.to_buffer(recon(8..14));
    assert_eq!(
        mapped.len(),
        2,
        "a cross-line range must split into two pieces"
    );

    assert_eq!(mapped[0].line(), 0);
    assert_eq!(mapped[0].range(), 8..11);
    assert_eq!(&content[mapped[0].range()], "1;\n");

    assert_eq!(mapped[1].line(), 1);
    assert_eq!(mapped[1].range(), 13..16);
    assert_eq!(&content[mapped[1].range()], "let");

    let combined: String = mapped.iter().map(|p| &content[p.range()]).collect();
    assert_eq!(combined, "1;\nlet");
    assert!(
        !combined.contains("> "),
        "the pieces must never include the blockquote's own gap bytes"
    );
}

/// The CRLF defect at its own layer: a physical line's raw range still
/// carries its trailing `\r`, and no piece `to_buffer` returns may ever
/// include it — checked here directly against `LineMap`, one level below the
/// end-to-end highlight pipeline the integration gate checks.
#[test]
fn to_buffer_never_lets_a_trimmed_carriage_return_back_into_a_piece() {
    let content = "one\r\ntwo\r\n";
    let map = LineMap::new(content, vec![0..4, 5..9]);
    assert_eq!(map.reconstruct(content).unwrap(), "one\ntwo");

    let mapped = map.to_buffer(recon(0..7));
    assert_eq!(mapped.len(), 2);
    for piece in &mapped {
        let text = &content[piece.range()];
        assert!(!text.contains('\r'), "piece text {text:?} carries a \\r");
    }
    let combined: String = mapped.iter().map(|p| &content[p.range()]).collect();
    assert_eq!(combined, "onetwo");
}

#[test]
fn to_reconstructed_maps_a_non_final_line_end_to_the_joining_newline() {
    let (_, map) = nested();
    // Line 0 ends at buffer offset 10, which is the buffer's real '\n'
    // and the reconstructed text's joining '\n' at offset 10.
    assert_eq!(map.to_reconstructed(buf(10..11)), Some(recon(10..11)));
}

#[test]
fn to_reconstructed_rejects_container_prefix_bytes() {
    let (_, map) = nested();
    // Buffer 11..13 is "> ", pure gap: no reconstructed counterpart, and
    // certainly not line 1's opening offset.
    assert_eq!(map.to_reconstructed(buf(11..12)), None);
    assert_eq!(map.to_reconstructed(buf(12..13)), None);
    // The byte right after the last line's end is past the text.
    assert_eq!(map.to_reconstructed(buf(23..24)), None);
}

#[test]
fn to_reconstructed_rejects_offsets_before_the_first_line() {
    let map = one_line("01234567", 5..8);
    assert_eq!(map.to_reconstructed(buf(0..1)), None);
    assert_eq!(map.to_reconstructed(buf(4..5)), None);
    assert_eq!(map.to_reconstructed(buf(5..6)), Some(recon(0..1)));
}

#[test]
fn empty_and_inverted_ranges_map_nowhere() {
    let (_, map) = contiguous();
    // Spelled as struct literals: a bare `3..3`/`5..2` is a hard clippy
    // error, and these degenerate inputs are exactly what is under test.
    let empty = Range { start: 3, end: 3 };
    let inverted = Range { start: 5, end: 2 };
    assert!(map.to_buffer(recon(empty.clone())).is_empty());
    assert!(map.to_buffer(recon(inverted.clone())).is_empty());
    assert_eq!(map.to_reconstructed(buf(empty)), None);
    assert_eq!(map.to_reconstructed(buf(inverted)), None);
}

#[test]
fn an_empty_line_map_maps_nothing() {
    let map = LineMap::new("", vec![]);
    assert_eq!(map.reconstruct("anything").unwrap(), "");
    assert!(map.to_buffer(recon(0..1)).is_empty());
    assert_eq!(map.to_reconstructed(buf(0..1)), None);
}

#[test]
fn a_single_line_maps_its_own_content_and_nothing_past_it() {
    let content = "abc";
    let map = one_line(content, 0..3);
    assert_eq!(map.reconstruct(content).unwrap(), "abc");
    let mapped = map.to_buffer(recon(0..3));
    assert_eq!(mapped.len(), 1);
    assert_eq!(mapped[0].range(), 0..3);
    // A single line is also the LAST line, so it has no joining '\n':
    // offset 3 is past the reconstructed text in both directions.
    assert!(map.to_buffer(recon(3..4)).is_empty());
    assert_eq!(map.to_reconstructed(buf(3..4)), None);
}

#[test]
fn a_blank_line_in_the_middle_still_carries_its_joining_newline() {
    // "a\n\nb": line 1 is empty and contributes only the '\n' that joins
    // it to line 2.
    let content = "a\n\nb";
    let map = LineMap::new(content, vec![0..1, 2..2, 3..4]);
    assert_eq!(map.reconstruct(content).unwrap(), "a\n\nb");

    // The blank line's own newline sits at buffer offset 2 and
    // reconstructed offset 2.
    let mapped = map.to_buffer(recon(2..3));
    assert_eq!(mapped.len(), 1);
    assert_eq!(mapped[0].range(), 2..3);
    assert_eq!(map.to_reconstructed(buf(2..3)), Some(recon(2..3)));
}

/// The property the two directions exist to guarantee: every in-range
/// reconstructed range survives a round trip through buffer coordinates
/// unchanged — piece by piece, since a range crossing a line boundary
/// now comes back as several. Run over both shapes a fence can take —
/// buffer-contiguous and container-nested — since only the nested one
/// exercises the gaps.
#[test]
fn every_reconstructed_range_round_trips_through_buffer_coordinates() {
    for (content, map) in [contiguous(), nested()] {
        let text = map.reconstruct(content).unwrap();
        for start in 0..text.len() {
            for end in (start + 1)..=text.len() {
                let r = start..end;
                let pieces = map.to_buffer(recon(r.clone()));
                assert!(
                    !pieces.is_empty(),
                    "every in-range reconstructed range maps to the buffer"
                );
                let mut cursor = r.start;
                for piece in &pieces {
                    let back = map
                        .to_reconstructed(buf(piece.range()))
                        .expect("a mapped piece must map back");
                    assert_eq!(back.start.0, cursor, "pieces must cover {r:?} contiguously");
                    cursor = back.end.0;
                }
                assert_eq!(cursor, r.end, "pieces must cover the whole of {r:?}");
            }
        }
    }
}

/// The render path's window translation: an arbitrary viewport slice
/// widens to whole lines rather than reporting `None` the way the exact
/// `to_reconstructed` does for an endpoint sitting in a container
/// prefix.
#[test]
fn reconstructed_window_widens_a_gap_landing_window_to_whole_lines() {
    let (_, map) = nested();
    // Buffer 9..15 starts inside line 0 and ends inside line 1, crossing
    // the "> " prefix between them. `to_reconstructed` refuses the
    // prefix bytes outright; the window widens to both whole lines.
    assert_eq!(map.to_reconstructed(buf(11..12)), None);
    assert_eq!(map.reconstructed_window(buf(9..15)), Some(recon(0..21)));
}

/// A window falling ENTIRELY inside a container prefix intersects no
/// line and therefore covers nothing — widening never invents bytes.
#[test]
fn reconstructed_window_reports_none_for_a_window_wholly_inside_a_gap() {
    let (_, map) = nested();
    assert_eq!(map.reconstructed_window(buf(11..13)), None);
}

#[test]
fn reconstructed_window_covers_the_whole_text_for_an_oversized_window() {
    let (content, map) = nested();
    let len = map.reconstruct(content).unwrap().len();
    assert_eq!(map.reconstructed_window(buf(0..1000)), Some(recon(0..len)));
}

#[test]
fn reconstructed_window_reports_none_when_no_line_intersects() {
    let map = one_line("01234567", 5..8);
    assert_eq!(map.reconstructed_window(buf(0..5)), None);
    assert_eq!(map.reconstructed_window(buf(9..20)), None);
    let empty = Range { start: 6, end: 6 };
    assert_eq!(map.reconstructed_window(buf(empty)), None);
    assert_eq!(
        LineMap::new("", vec![]).reconstructed_window(buf(0..10)),
        None
    );
}

/// A window covering only the second line must not drag the first
/// line's bytes in with it — the widening is to whole INTERSECTING
/// lines, never to the region's whole extent.
#[test]
fn reconstructed_window_starts_at_the_first_intersecting_line() {
    let (_, map) = nested();
    assert_eq!(map.reconstructed_window(buf(15..18)), Some(recon(11..21)));
}

#[test]
fn an_out_of_range_reconstructed_offset_maps_nowhere() {
    let (content, map) = nested();
    let len = map.reconstruct(content).unwrap().len();
    assert!(map.to_buffer(recon(len..len + 1)).is_empty());
    assert!(map.to_buffer(recon(len + 100..len + 101)).is_empty());
}
