//! Tests for `DocMachine`, split out to keep the owning module under
//! CONSTITUTION §1.6's 500-LoC limit.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::*;
use rune_core::cursor::CursorSet;

#[test]
fn set_focus_is_idempotent_and_marks_dirty_only_on_change() {
    let mut doc = DocMachine::new();
    assert_eq!(doc.state(), DocState::Unfocused);
    doc.clear_dirty();
    doc.set_focus(false);
    assert!(!doc.is_dirty(), "no-op focus change must not dirty");
    doc.set_focus(true);
    assert!(doc.is_dirty());
    assert_eq!(doc.state(), DocState::Focused);
}

#[test]
fn sync_content_is_a_true_no_op_when_version_is_unchanged() {
    let mut doc = DocMachine::new();
    doc.set_focus(true); // Decide policies only fire when focused.
    let buf = Buffer::new("# hello\n");
    doc.sync_content(&buf);
    assert_eq!(doc.built_version, buf.version());
    assert!(!doc.blocks().is_empty());

    // Reveal the heading (cursor on its line), so its `RevealSm` is now
    // `Revealed` — a state that lives ONLY on the current `blocks` Vec.
    let cursors = CursorSet::new(0);
    doc.sync_cursors(&buf, &cursors);
    assert_eq!(
        doc.blocks()[0].reveal_state(),
        rune_syntax::element::RevealState::Revealed
    );

    // Calling sync_content again with the SAME version must be a true
    // no-op: if it silently reparsed, the freshly-built Heading machine
    // would reset to its default Rendered state, discarding the reveal
    // transition above without ever bumping `built_version` — a `Vec`
    // identity check can't catch this (a Vec can get a new backing
    // allocation with byte-identical contents), but the reveal state
    // survives if and only if no reparse actually happened.
    doc.sync_content(&buf);
    assert_eq!(
        doc.blocks()[0].reveal_state(),
        rune_syntax::element::RevealState::Revealed,
        "sync_content must not reparse when buf.version() is unchanged"
    );
}

#[test]
fn snapshot_short_circuits_when_nothing_changed_between_two_view_calls() {
    // The keystroke-latency regression this test guards: `view()` may be
    // called several times per message batch by sanctioned design, and a
    // cursor-only move changes none of `sync_content`/`set_width`/
    // `sync_cursors`/`set_focus`'s inputs — the second `snapshot` call
    // must be a memo hit, not a second emit + wrap + `expand_tables`.
    let mut doc = DocMachine::new();
    doc.set_focus(true);
    let buf = Buffer::new("# hello\nworld\n");
    let cursors = CursorSet::new(0);

    doc.sync_content(&buf);
    doc.set_width(80);
    doc.sync_cursors(&buf, &cursors);
    let first = doc.snapshot(&buf);
    assert_eq!(doc.rebuild_count(), 1);

    // Same version, same width, same cursor/reveal state: the whole
    // per-message sync sequence again, exactly as `Document::view` would
    // run it for a second call within the same batch.
    doc.sync_content(&buf);
    doc.set_width(80);
    doc.sync_cursors(&buf, &cursors);
    let second = doc.snapshot(&buf);
    assert_eq!(
        doc.rebuild_count(),
        1,
        "a second view() call with no changed input must be a memo hit"
    );
    assert_eq!(first.display.total_rows(), second.display.total_rows());

    // Sanity: a real input change (width) still forces a rebuild.
    doc.set_width(40);
    doc.sync_cursors(&buf, &cursors);
    doc.snapshot(&buf);
    assert_eq!(
        doc.rebuild_count(),
        2,
        "a genuine width change must still force a rebuild"
    );
}

#[test]
fn sync_cursors_never_bumps_built_version() {
    let mut doc = DocMachine::new();
    let buf = Buffer::new("# hello\nworld\n");
    doc.sync_content(&buf);
    let before = doc.built_version;
    let cursors = CursorSet::new(0);
    doc.sync_cursors(&buf, &cursors);
    assert_eq!(doc.built_version, before, "reveal must never bump version");
}

#[test]
fn unfocused_forces_every_decide_policy_block_rendered() {
    // This fixture has only a Heading — a `Decide`-policy block, whose
    // reveal follows `ctx.grant`. It does NOT cover Frontmatter/
    // Verbatim, which are pinned Revealed by design regardless of
    // focus (the reveal-policy table: "Frontmatter, Verbatim | pinned
    // Revealed (no Decide)") — see
    // `frontmatter_and_verbatim_survive_unfocused_as_revealed` below
    // for that intentional exception.
    let mut doc = DocMachine::new();
    let buf = Buffer::new("# hello\n");
    doc.sync_content(&buf);
    // cursor sits on the heading line, which WOULD reveal if focused.
    let cursors = CursorSet::new(2);
    doc.sync_cursors(&buf, &cursors);
    for b in doc.blocks() {
        assert_eq!(
            b.reveal_state(),
            rune_syntax::element::RevealState::Rendered,
            "unfocused doc must force every Decide-policy block Rendered"
        );
    }
}

#[test]
fn frontmatter_and_verbatim_survive_unfocused_as_revealed() {
    // The reveal-policy table's declared exception to "Unfocused ->
    // ForceRendered": Frontmatter and Verbatim (HTML/math/any other
    // unmodeled construct) have no Decide policy at all — they ignore
    // `ctx.grant` entirely and stay pinned Revealed even when the
    // document is unfocused.
    //
    // A table is no longer part of this exception (plan: markdown
    // table rendering, WP1): `Block::Table` now has a real Decide
    // policy (`cursors.any_in_lines(first_line, last_line)`, mirroring
    // `CodeFenceM`), so an unfocused document forces it Rendered like
    // every other Decide-policy block. The fixture below therefore uses
    // an HTML block, which is still a pinned-Revealed `Verbatim`.
    let mut doc = DocMachine::new();
    let buf = Buffer::new("---\ntitle: x\n---\n\n<div>\nraw\n</div>\n");
    doc.sync_content(&buf);
    doc.sync_cursors(&buf, &CursorSet::new(0));
    assert!(
        doc.blocks().len() >= 2,
        "expected a Frontmatter block and a Verbatim (html) block"
    );
    for b in doc.blocks() {
        assert!(
            matches!(
                b,
                crate::element::block::Block::Frontmatter(_)
                    | crate::element::block::Block::Verbatim(_)
            ),
            "unexpected block kind in this fixture: {b:?}"
        );
        assert_eq!(
            b.reveal_state(),
            rune_syntax::element::RevealState::Revealed
        );
    }
}

/// Every block reached by `reveal_all` (including nested ones, which
/// `Block::reveal_state` alone can't see through a `Blockquote`'s or
/// `List`'s own composite `reveal_state`) reports `Revealed` — this is
/// what makes fence-body emission (WP6.S3) always render at full
/// reveal regardless of the fixture's own Decide policies.
fn assert_all_revealed(blocks: &[crate::element::block::Block]) {
    use crate::element::block::Block;
    use rune_syntax::element::RevealState;
    for b in blocks {
        // `Paragraph` carries no marker of its own — `Block::reveal_state`
        // always reports it `Rendered` by design (it is never itself a
        // conceal target); skip it here rather than assert on a report
        // that can never be anything else.
        if !matches!(b, Block::Paragraph(_)) {
            assert_eq!(
                b.reveal_state(),
                RevealState::Revealed,
                "block not revealed: {b:?}"
            );
        }
        match b {
            Block::Blockquote(m) => {
                for marker in &m.markers {
                    assert_eq!(marker.sm.state(), RevealState::Revealed);
                }
                assert_all_revealed(&m.children);
            }
            Block::List(m) => {
                for item in &m.items {
                    assert_eq!(item.sm.state(), RevealState::Revealed);
                    assert_all_revealed(&item.children);
                }
            }
            _ => {}
        }
    }
}

#[test]
fn reveal_all_forces_every_block_revealed_with_no_cursor_input() {
    // Deliberately unfocused-shaped input (no `DocMachine`, no
    // `CursorSet` at all): a heading, a blockquote wrapping a nested
    // list, a fenced code block, and frontmatter — every one of these
    // has cursor-line/cursor-range Decide policies that would normally
    // stay Rendered with the cursor elsewhere, plus a pinned-Revealed
    // Frontmatter block to confirm `reveal_all` doesn't disturb it.
    let content = "---\ntitle: x\n---\n\n# Heading\n\n> quote line\n> - item one\n> - item two\n\n```rust\nfn f() {}\n```\n";
    let mut blocks = crate::parse::parse(content);
    reveal_all(&mut blocks);
    assert!(!blocks.is_empty(), "fixture must produce parsed blocks");
    assert_all_revealed(&blocks);
}
