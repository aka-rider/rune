use super::*;
use crate::element::block::Block;

/// `~~text~~` must classify as `EmphasisKind::Strike`, not fall through to
/// `InlineKind::TextLike` the way an unmodeled inline node degrades.
#[test]
fn strikethrough_parses_to_its_own_emphasis_kind() {
    let content = "~~gone~~\n";
    let blocks = crate::parse::parse(content);
    let Block::Paragraph(p) = &blocks[0] else {
        panic!("expected paragraph");
    };
    let Inline::Emphasis(m) = &p.inlines[0] else {
        panic!("expected strikethrough emphasis, got {:?}", p.inlines[0]);
    };
    assert_eq!(m.kind, EmphasisKind::Strike);
}

/// `LinkM::url_range` names the exact byte span of the URL between the
/// `](` and `)` delimiters — not a default/empty range.
#[test]
fn link_url_range_names_the_exact_url_bytes() {
    let content = "[text](http://x)\n";
    let blocks = crate::parse::parse(content);
    let Block::Paragraph(p) = &blocks[0] else {
        panic!("expected paragraph");
    };
    let Inline::Link(link) = &p.inlines[0] else {
        panic!("expected link, got {:?}", p.inlines[0]);
    };
    assert_eq!(
        content.get(link.url_range.start..link.url_range.end),
        Some("http://x")
    );
}

#[test]
fn leading_backtick_run_refuses_a_run_longer_than_want() {
    let content = "```x";
    let line = ByteRange::new(0, content.len());
    assert_eq!(leading_backtick_run(content, line, 2), None);
}

#[test]
fn leading_backtick_run_accepts_an_exact_run() {
    let content = "``x";
    let line = ByteRange::new(0, content.len());
    assert_eq!(
        leading_backtick_run(content, line, 2),
        Some(ByteRange::new(0, 2))
    );
}

#[test]
fn trailing_backtick_run_refuses_when_no_run_of_want_length_exists_in_bounds() {
    let content = "`x``y";
    assert_eq!(trailing_backtick_run(content, 1, content.len(), 1), None);
}

#[test]
fn trailing_backtick_run_finds_the_first_matching_run() {
    let content = "`ab`cd`";
    assert_eq!(
        trailing_backtick_run(content, 1, content.len(), 1),
        Some(ByteRange::new(3, 4))
    );
}

#[test]
fn trailing_backtick_run_never_scans_past_its_limit() {
    let content = "`ab`cd`";
    assert_eq!(trailing_backtick_run(content, 1, 3, 1), None);
}

/// A run that IS the whole scanned span, matching `want` exactly, must be
/// found without ever falling back to the byte-at-a-time "not a backtick"
/// skip path — this is the one shape that still terminates (and so can
/// still distinguish a mutant) when that skip path's own `!=`/`==` flips,
/// because the scan never needs to take it: the very first byte already
/// starts a matching run.
#[test]
fn trailing_backtick_run_finds_a_run_that_fills_the_whole_span() {
    assert_eq!(
        trailing_backtick_run("```", 0, 3, 3),
        Some(ByteRange::new(0, 3))
    );
}

/// `want == 0` is a degenerate call no real code span makes, but it is the
/// one input that still terminates (and so can still distinguish a mutant)
/// for the INNER run-measuring loop's own comparison: the genuine loop
/// always consumes AT LEAST the one backtick that led it in, so a
/// zero-length "run" can never actually satisfy `want == 0` — a mutant
/// whose inner condition stops the loop BEFORE consuming that backtick
/// would instead match immediately.
#[test]
fn trailing_backtick_run_of_zero_length_never_matches_a_real_backtick() {
    assert_eq!(trailing_backtick_run("`ab`cd`", 1, 7, 0), None);
}

/// The inner run-measuring loop must stop exactly AT `limit`, never
/// consuming a byte sitting there even when it too is a backtick — this
/// fixture's real 1-backtick run ends exactly at `limit`, immediately
/// followed by one more backtick just past it that must never be folded
/// into the same run.
#[test]
fn trailing_backtick_run_never_consumes_a_backtick_sitting_at_its_limit() {
    assert_eq!(
        trailing_backtick_run("`ab``", 1, 4, 1),
        Some(ByteRange::new(3, 4))
    );
}

/// The leading scan must stop exactly AT `line.end`, never reading a byte
/// sitting there even when it too is a backtick: `line` here is narrowed to
/// end one byte before the run's true (content-level) close, so a scan
/// that overruns it would read a run one backtick too long and refuse the
/// match this fixture expects to succeed.
#[test]
fn leading_backtick_run_never_reads_past_its_own_line_end() {
    let content = "```x";
    let line = ByteRange::new(0, 2);
    assert_eq!(
        leading_backtick_run(content, line, 2),
        Some(ByteRange::new(0, 2))
    );
}

/// `build_inlines`'s multiline-wikilink recovery rebuilds each continuation
/// line from `hint.start_for_line`, guarded by `s < e` against a hint whose
/// claim overshoots (or exactly meets) that line's own end — the same
/// "producer hint outran the real line" shape `per_line_content_clamps_a_
/// hint_start_that_overshoots_its_own_line` (above) pins for the sibling
/// per-line splitter, reached here through a HAND-BUILT `ScanHint` rather
/// than real content: no genuine `parse()` input in this comrak version
/// drives a real container hint that far off (the CLASS A comrak quirk the
/// surrounding comment describes appears to no longer reproduce), so the
/// guard's own contract is pinned directly instead. Two hints, both
/// expected to skip line 1 ("]]") and still recover line 2 ("b") exactly
/// as an ordinary (non-overshooting) hint would:
/// - `overshoot`: the hint claims past line 1's own end (`s > e`).
/// - `tie`: the hint claims EXACTLY line 1's own end (`s == e`) — nothing
///   real is left to recover, so this must skip too, not emit an empty
///   piece.
#[test]
fn multiline_wikilink_recovery_skips_a_line_a_hint_claims_past_or_up_to_its_end() {
    use comrak::{Arena, parse_document};
    let content = "[[\n]]\nb\n";
    let starts = super::super::line_starts(content);
    let arena = Arena::new();
    let opts = super::super::options();
    let root = parse_document(&arena, content, &opts);
    let paragraph = root.first_child().expect("paragraph");
    let expected = vec![(0, 2), (6, 7)];
    let text_ranges = |inlines: &[Inline]| -> Vec<(usize, usize)> {
        inlines
            .iter()
            .map(|i| {
                let r = i.range();
                (r.start, r.end)
            })
            .collect()
    };

    let overshoot_hint = ScanHint::Nested {
        marker_ends: std::collections::HashMap::from([(1, 10)]),
        conceals_own_prefix: true,
        parent: &ScanHint::Root,
    };
    assert_eq!(
        text_ranges(&build_inlines(content, &starts, paragraph, &overshoot_hint)),
        expected
    );

    let tie_hint = ScanHint::Nested {
        marker_ends: std::collections::HashMap::from([(1, 5)]),
        conceals_own_prefix: true,
        parent: &ScanHint::Root,
    };
    assert_eq!(
        text_ranges(&build_inlines(content, &starts, paragraph, &tie_hint)),
        expected
    );
}
