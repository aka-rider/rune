#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use super::*;
use crate::element::block::Block;
use crate::element::inline::{EmphasisKind, Inline};
use rune_syntax::element::RevealState;

fn text_of(content: &str, r: ByteRange) -> &str {
    content.get(r.start..r.end).unwrap()
}

/// `options_without_frontmatter` is `options()` with only the frontmatter
/// extension turned off — never a blank slate. A `Default::default()`
/// stand-in would silently drop strikethrough/tasklist/table/wikilink/
/// autolink support too, the one difference this pins directly: the
/// fallback this builds is only ever reached for a document shape no
/// currently-pinned `parse()` fixture can construct (see
/// `frontmatter::frontmatter_extension_is_safe`'s own docs on the
/// comrak CRLF quirk it exists to catch), so its own contract is pinned
/// here rather than through a full `parse()` round trip.
#[test]
fn options_without_frontmatter_disables_only_the_frontmatter_extension() {
    let with = options();
    let without = options_without_frontmatter();
    assert!(with.extension.front_matter_delimiter.is_some());
    assert!(without.extension.front_matter_delimiter.is_none());
    assert!(without.extension.strikethrough);
    assert!(without.extension.tasklist);
    assert!(without.extension.table);
    assert!(without.extension.wikilinks_title_after_pipe);
    assert!(without.extension.autolink);
}

#[test]
fn heading_marker_and_text_are_byte_exact() {
    let content = "## heading\n";
    let blocks = parse(content);
    assert_eq!(blocks.len(), 1);
    let Block::Heading(h) = &blocks[0] else {
        panic!("expected heading");
    };
    assert_eq!(h.level, 2);
    assert_eq!(text_of(content, h.marker), "## ");
    assert_eq!(text_of(content, h.range), "## heading");
}

#[test]
fn bold_delimiters_and_nested_link() {
    let content = "**[bo*ld*](url)**\n";
    let blocks = parse(content);
    let Block::Paragraph(p) = &blocks[0] else {
        panic!("expected paragraph");
    };
    let Inline::Emphasis(bold) = &p.inlines[0] else {
        panic!("expected bold emphasis");
    };
    assert_eq!(bold.kind, EmphasisKind::Bold);
    assert_eq!(text_of(content, bold.open), "**");
    assert_eq!(text_of(content, bold.close), "**");
    let Inline::Link(link) = &bold.children[0] else {
        panic!("expected link");
    };
    assert_eq!(link.url, "url");
    assert_eq!(text_of(content, link.range), "[bo*ld*](url)");
}

#[test]
fn fenced_code_block_fences_and_content() {
    let content = "```rust\nfn f() {}\n```\n";
    let blocks = parse(content);
    let Block::CodeFence(cf) = &blocks[0] else {
        panic!("expected code fence");
    };
    assert_eq!(cf.language, "rust");
    assert_eq!(text_of(content, cf.fence_open), "```rust");
    assert_eq!(text_of(content, cf.fence_close.unwrap()), "```");
    // One range per content line (never one contiguous span — see
    // CodeFenceM's docs), each excluding its own trailing `\n` like
    // every other per-line range in this crate.
    assert_eq!(cf.content_lines.len(), 1);
    assert_eq!(text_of(content, cf.content_lines[0]), "fn f() {}");
}

/// An UNTERMINATED fence (no closing ` ``` `) has no `close` line, so
/// `delimited::split`'s body range must still reach all the way to the
/// LAST line comrak's own range covers, inclusive — not stop one line
/// short of it.
#[test]
fn an_unterminated_fence_keeps_its_last_content_line() {
    let content = "```rust\nline one\nline two\n";
    let blocks = parse(content);
    let Block::CodeFence(cf) = &blocks[0] else {
        panic!("expected code fence");
    };
    assert!(cf.fence_close.is_none(), "fence must be unterminated");
    assert_eq!(cf.content_lines.len(), 2);
    assert_eq!(text_of(content, cf.content_lines[0]), "line one");
    assert_eq!(text_of(content, cf.content_lines[1]), "line two");
}

#[test]
fn blockquote_marker_per_line() {
    let content = "> line one\n> line two\n";
    let blocks = parse(content);
    let Block::Blockquote(bq) = &blocks[0] else {
        panic!("expected blockquote");
    };
    assert_eq!(bq.markers.len(), 2);
    assert_eq!(text_of(content, bq.markers[0].marker), "> ");
    assert_eq!(bq.markers[0].line, 0);
    assert_eq!(bq.markers[1].line, 1);
}

/// A run of 2500 nested `>` markers has no cap in comrak itself (unlike
/// its own 100-deep LIST cap) — `build_block`'s `MAX_CONTAINER_DEPTH`
/// guard is the only thing standing between this and a stack overflow:
/// unbounded recursion through `build_block`/`build_blocks` (parse), the
/// mirrored recursive walk in `emit::emit`, `catalogue::catalogue`, and
/// the compiler-generated recursive `Drop` glue for the `Block` tree
/// itself (`Vec<Block>` dropping each nested `Blockquote`'s own
/// `Vec<Block>` children) all consume the SAME tree this depth cap
/// bounds, so exercising all three here on one thread proves every one
/// of them stays within a real stack rather than just the parse walk.
/// Spawned on its own thread with an explicit, platform-typical 8 MiB
/// stack rather than trusting the test harness's own worker thread,
/// which can default to a smaller stack than a real terminal session's
/// main thread ever would.
#[test]
fn a_pathologically_deep_blockquote_does_not_overflow_the_stack() {
    let content = format!("{} x\n", ">".repeat(2500));
    let handle = std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(move || {
            let blocks = parse(&content);
            let (_, _) = crate::emit::emit(&content, &blocks, 80);
            let _ = crate::catalogue::catalogue(&content, &blocks);
            blocks.len()
        })
        .expect("spawn the repro thread");
    let top_level_blocks = handle
        .join()
        .expect("parse/emit/catalogue/drop must not abort the thread");
    assert_eq!(
        top_level_blocks, 1,
        "expected one top-level blockquote, its innermost nesting flattened to raw content"
    );
}

#[test]
fn tasklist_marker_and_task_range() {
    let content = "- [x] task\n";
    let blocks = parse(content);
    let Block::List(list) = &blocks[0] else {
        panic!("expected list");
    };
    assert_eq!(list.items.len(), 1);
    let item = &list.items[0];
    assert_eq!(text_of(content, item.marker), "- [x] ");
    assert_eq!(text_of(content, item.task.unwrap()), "[x]");
}

#[test]
fn wikilink_target_and_label() {
    let content = "[[wiki|label]]\n";
    let blocks = parse(content);
    let Block::Paragraph(p) = &blocks[0] else {
        panic!("expected paragraph");
    };
    let Inline::WikiLink(w) = &p.inlines[0] else {
        panic!("expected wikilink");
    };
    assert_eq!(w.target, "wiki");
    assert_eq!(text_of(content, w.label), "label");
}

#[test]
fn table_and_html_block_become_verbatim() {
    // HTML blocks still degrade to raw passthrough (unknown syntax
    // degrades to visible raw text, never lost).
    let html = "<div>\nraw\n</div>\n";
    let blocks = parse(html);
    assert!(matches!(blocks[0], Block::Verbatim(_)));

    // A table now parses into a real element machine instead — see
    // `table_model.rs` for coverage of its shape.
    let table = "| a | b |\n|---|---|\n| 1 | 2 |\n";
    let blocks = parse(table);
    assert!(matches!(blocks[0], Block::Table(_)));
}

#[test]
fn frontmatter_is_pinned_revealed() {
    let content = "---\ntitle: x\n---\nbody\n";
    let blocks = parse(content);
    let Block::Frontmatter(fm) = &blocks[0] else {
        panic!("expected frontmatter, got {:?}", blocks[0]);
    };
    assert_eq!(fm.sm.state(), RevealState::Revealed);
}

#[test]
fn a_document_without_a_frontmatter_opener_parses_identically() {
    let content = "# heading\n\nbody text\n";
    assert!(!frontmatter::shadow_may_open_frontmatter(&parse_shadow(
        content
    )));
    let blocks = parse(content);
    assert_eq!(blocks.len(), 2);
    assert!(matches!(blocks[0], Block::Heading(_)));
    assert!(matches!(blocks[1], Block::Paragraph(_)));
}

#[test]
fn per_line_content_clamps_a_hint_start_that_overshoots_its_own_line() {
    let content = "x".repeat(15);
    let starts = vec![0, 5, 10];
    let range = ByteRange::new(0, 15);
    let hint = ScanHint::Nested {
        marker_ends: std::collections::HashMap::from([(1, 12)]),
        conceals_own_prefix: true,
        parent: &ScanHint::Root,
    };
    let lines = per_line_content(&content, &starts, range, &hint);
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[1], ByteRange::new(9, 9));
}
