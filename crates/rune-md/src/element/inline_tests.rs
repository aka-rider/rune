//! Tests for the per-inline `sync`/accessor machinery, split out to keep
//! `inline.rs` under the 500-line budget. Follows `block_tests.rs`'s own
//! shape: hand-built `InheritCtx`s drive the private `sync` methods through
//! the public `Inline::sync` entry point, isolating aggregation from the
//! cursor/line arithmetic that decides any one child's own `want`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::*;
use rune_syntax::element::{CursorProbe, RevealGrant, WrapState};

fn wiki_link(range: ByteRange) -> Inline {
    Inline::WikiLink(WikiLinkM {
        sm: RevealSm::new(RevealState::Rendered),
        range,
        line: 0,
        target: String::new(),
        label: ByteRange::new(0, 0),
    })
}

#[test]
fn emphasis_sync_reports_dirty_from_a_child_even_when_its_own_state_is_unchanged() {
    let wrap = WrapState::default();
    let cursors = CursorProbe::default();
    let ctx = InheritCtx {
        wrap: &wrap,
        grant: RevealGrant::ForceRevealed,
        cursors: &cursors,
    };
    let mut emphasis = Inline::Emphasis(EmphasisM {
        sm: RevealSm::new(RevealState::Revealed), // already at the forced state: its own transition is a no-op
        kind: EmphasisKind::Bold,
        range: ByteRange::new(0, 8),
        open: ByteRange::new(0, 2),
        close: ByteRange::new(6, 8),
        children: vec![wiki_link(ByteRange::new(2, 6))],
        line: 0,
        content_lines: Vec::new(),
    });
    assert!(
        emphasis.sync(&ctx),
        "an emphasis node's own no-op transition must not swallow a child's dirty transition"
    );
}

#[test]
fn link_sync_reports_dirty_from_a_child_even_when_its_own_state_is_unchanged() {
    let wrap = WrapState::default();
    let cursors = CursorProbe::default();
    let ctx = InheritCtx {
        wrap: &wrap,
        grant: RevealGrant::ForceRevealed,
        cursors: &cursors,
    };
    let mut link = Inline::Link(LinkM {
        sm: RevealSm::new(RevealState::Revealed),
        range: ByteRange::new(0, 8),
        line: 0,
        text: vec![wiki_link(ByteRange::new(1, 7))],
        url: String::new(),
        url_range: ByteRange::new(0, 0),
        content_lines: Vec::new(),
    });
    assert!(
        link.sync(&ctx),
        "a link's own no-op transition must not swallow a child's dirty transition"
    );
}

#[test]
fn inline_code_content_lines_and_inner_lines_are_the_real_derived_ranges() {
    let code =
        InlineCodeM::between_delimiters(ByteRange::new(2, 3), ByteRange::new(5, 6), |r| vec![r]);
    assert_eq!(
        code.content_lines(),
        &[ByteRange::new(2, 6)],
        "content_lines must be per_line(open.start..close.end), not an empty or default-filled leak"
    );
    assert_eq!(
        code.inner_lines(),
        &[ByteRange::new(3, 5)],
        "inner_lines must be per_line(open.end..close.start), not an empty or default-filled leak"
    );
}
