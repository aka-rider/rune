//! Tests for the per-block `sync`/`range` machinery, split out to keep
//! `block.rs` under the 500-line budget. Every `sync` test below drives the
//! private per-variant `sync` method directly (through the public
//! `Block::sync`/`Inline::sync` entry points) with a hand-built `InheritCtx`
//! rather than a parsed document, so the aggregation logic (`dirty |= ...`
//! across a variant's children) is isolated from the cursor/line arithmetic
//! that decides any one child's own `want`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::*;
use crate::element::inline::LinkM;
use rune_syntax::element::{CursorProbe, RevealGrant, WrapState};

fn force_revealed_ctx() -> (WrapState, CursorProbe) {
    (WrapState::default(), CursorProbe::default())
}

fn rendered_link(range: ByteRange) -> Inline {
    Inline::Link(LinkM {
        sm: RevealSm::new(RevealState::Rendered),
        range,
        line: 0,
        text: Vec::new(),
        url: String::new(),
        url_range: ByteRange::new(0, 0),
        content_lines: Vec::new(),
    })
}

#[test]
fn paragraph_sync_reports_dirty_when_its_inline_transitions() {
    let (wrap, cursors) = force_revealed_ctx();
    let ctx = InheritCtx {
        wrap: &wrap,
        grant: RevealGrant::ForceRevealed,
        cursors: &cursors,
    };
    let mut block = Block::Paragraph(ParagraphM {
        range: ByteRange::new(0, 5),
        inlines: vec![rendered_link(ByteRange::new(0, 5))],
    });
    assert!(
        block.sync(&ctx),
        "a child inline's own reveal transition must propagate as dirty"
    );
}

#[test]
fn heading_sync_reports_dirty_from_a_child_even_when_its_own_state_is_unchanged() {
    let (wrap, cursors) = force_revealed_ctx();
    let ctx = InheritCtx {
        wrap: &wrap,
        grant: RevealGrant::ForceRevealed,
        cursors: &cursors,
    };
    let mut block = Block::Heading(HeadingM {
        sm: RevealSm::new(RevealState::Revealed), // already at the forced state: its own transition is a no-op
        level: 1,
        line: 0,
        last_line: 0,
        range: ByteRange::new(0, 5),
        setext: false,
        marker: ByteRange::new(0, 2),
        underline: None,
        inlines: vec![rendered_link(ByteRange::new(2, 5))],
        content_lines: Vec::new(),
    });
    assert!(
        block.sync(&ctx),
        "a heading's own no-op transition must not swallow a child's dirty transition"
    );
}

#[test]
fn blockquote_sync_reports_dirty_from_a_marker_transition_alone() {
    let (wrap, cursors) = force_revealed_ctx();
    let ctx = InheritCtx {
        wrap: &wrap,
        grant: RevealGrant::ForceRevealed,
        cursors: &cursors,
    };
    let mut block = Block::Blockquote(BlockquoteM {
        range: ByteRange::new(0, 5),
        markers: vec![BlockquoteMarkerM {
            sm: RevealSm::new(RevealState::Rendered),
            line: 0,
            marker: ByteRange::new(0, 2),
        }],
        children: Vec::new(),
    });
    assert!(
        block.sync(&ctx),
        "a marker's own transition, with no children at all, must still be reported dirty"
    );
}

#[test]
fn blockquote_sync_reports_dirty_from_a_child_transition_alone() {
    let (wrap, cursors) = force_revealed_ctx();
    let ctx = InheritCtx {
        wrap: &wrap,
        grant: RevealGrant::ForceRevealed,
        cursors: &cursors,
    };
    let mut block = Block::Blockquote(BlockquoteM {
        range: ByteRange::new(0, 5),
        markers: Vec::new(),
        children: vec![Block::ThematicBreak(HrM {
            sm: RevealSm::new(RevealState::Rendered),
            line: 0,
            range: ByteRange::new(0, 3),
        })],
    });
    assert!(
        block.sync(&ctx),
        "a child block's own transition, with no markers at all, must still be reported dirty"
    );
}

#[test]
fn list_item_sync_reports_dirty_from_a_child_even_when_its_own_state_is_unchanged() {
    let (wrap, cursors) = force_revealed_ctx();
    let ctx = InheritCtx {
        wrap: &wrap,
        grant: RevealGrant::ForceRevealed,
        cursors: &cursors,
    };
    let mut item = ListItemM {
        sm: RevealSm::new(RevealState::Revealed),
        line: 0,
        marker: ByteRange::new(0, 2),
        task: None,
        children: vec![Block::ThematicBreak(HrM {
            sm: RevealSm::new(RevealState::Rendered),
            line: 0,
            range: ByteRange::new(2, 5),
        })],
    };
    assert!(
        item.sync(&ctx),
        "an item's own no-op transition must not swallow a child's dirty transition"
    );
}

#[test]
fn list_sync_reports_dirty_when_any_item_transitions() {
    let (wrap, cursors) = force_revealed_ctx();
    let ctx = InheritCtx {
        wrap: &wrap,
        grant: RevealGrant::ForceRevealed,
        cursors: &cursors,
    };
    let mut block = Block::List(ListM {
        ordered: false,
        items: vec![ListItemM {
            sm: RevealSm::new(RevealState::Rendered),
            line: 0,
            marker: ByteRange::new(0, 2),
            task: None,
            children: Vec::new(),
        }],
    });
    assert!(
        block.sync(&ctx),
        "a single item's own transition must be reported dirty"
    );
}

#[test]
fn frontmatter_sync_reports_the_real_transition_not_a_constant() {
    let (wrap, cursors) = force_revealed_ctx();
    let ctx = InheritCtx {
        wrap: &wrap,
        grant: RevealGrant::Decide, // FrontmatterM::sync ignores ctx entirely
        cursors: &cursors,
    };
    let mut block = Block::Frontmatter(FrontmatterM {
        sm: RevealSm::new(RevealState::Rendered),
        range: ByteRange::new(0, 8),
        first_line: 0,
        last_line: 2,
        open: ByteRange::new(0, 3),
        close: None,
        content_lines: Vec::new(),
    });
    assert!(
        block.sync(&ctx),
        "the first sync, from Rendered, must transition to Revealed and report true"
    );
    assert!(
        !block.sync(&ctx),
        "the second sync, already Revealed, must report false — not the same constant twice"
    );
}

#[test]
fn verbatim_sync_reports_the_real_transition_not_a_constant() {
    let (wrap, cursors) = force_revealed_ctx();
    let ctx = InheritCtx {
        wrap: &wrap,
        grant: RevealGrant::Decide,
        cursors: &cursors,
    };
    let mut block = Block::Verbatim(VerbatimM {
        sm: RevealSm::new(RevealState::Rendered),
        range: ByteRange::new(0, 8),
        kind: VerbatimKind::Html,
        content_lines: Vec::new(),
    });
    assert!(
        block.sync(&ctx),
        "the first sync, from Rendered, must transition to Revealed and report true"
    );
    assert!(
        !block.sync(&ctx),
        "the second sync, already Revealed, must report false — not the same constant twice"
    );
}

#[test]
fn block_range_is_computed_per_variant_not_a_fixed_default() {
    let paragraph = Block::Paragraph(ParagraphM {
        range: ByteRange::new(10, 20),
        inlines: Vec::new(),
    });
    assert_eq!(paragraph.range(), ByteRange::new(10, 20));
}

#[test]
fn list_range_spans_from_the_first_items_marker_to_the_last_items_end() {
    let list = Block::List(ListM {
        ordered: false,
        items: vec![ListItemM {
            sm: RevealSm::new(RevealState::Rendered),
            line: 0,
            marker: ByteRange::new(2, 4),
            task: None,
            children: Vec::new(),
        }],
    });
    assert_eq!(
        list.range(),
        ByteRange::new(2, 4),
        "a childless single item's range must come from its own marker, not Default::default()"
    );
}
