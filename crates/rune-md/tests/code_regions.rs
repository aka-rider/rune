//! What `ViewSnapshots::code_regions` promises, stated as a specification.
//!
//! A code region is the one definition of "a stretch of code" the rest of the
//! system reads. These tests pin the properties downstream consumers rely on:
//! that a region's content lines are container-prefix-free, that a region
//! exists even when nothing can highlight it, that a region's rows cover a
//! fence's delimiters, and that a whole code document is just another region.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_md::element::code_region::CodeRegion;
use rune_md::element::doc::DocMachine;
use rune_syntax::kind::DocumentKind;

/// Parse `content` as the given kind and return its code regions, read off
/// the display snapshot that publishes them — the same value every
/// production consumer reads.
fn regions_of(content: &str, kind: DocumentKind) -> (Buffer, Arc<[CodeRegion]>) {
    let buf = Buffer::new(content);
    let mut doc = DocMachine::new();
    doc.set_kind(kind);
    doc.sync_content(&buf);
    let regions = doc.snapshot(&buf).code_regions;
    (buf, regions)
}

fn markdown_regions(content: &str) -> (Buffer, Arc<[CodeRegion]>) {
    regions_of(content, DocumentKind::Markdown)
}

/// The text a consumer reconstructs from a region: each content line's own
/// slice, joined by a single newline. This is exactly what the highlight path
/// feeds a parser, so asserting on it asserts what a parser would actually
/// see.
fn reconstructed(buf: &Buffer, region: &CodeRegion) -> String {
    region
        .content
        .iter()
        .map(|r| buf.content().get(r.clone()).unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn a_top_level_fence_is_one_region_carrying_its_info_string() {
    let (buf, regions) = markdown_regions("```rust\nlet x = 1;\nlet y = 2;\n```\n");

    assert_eq!(regions.len(), 1, "one fence must yield exactly one region");
    let region = &regions[0];
    assert_eq!(region.info, "rust");
    assert_eq!(
        region.content.len(),
        2,
        "one range per physical content line"
    );
    assert_eq!(reconstructed(&buf, region), "let x = 1;\nlet y = 2;");
}

#[test]
fn a_fence_rows_span_covers_its_delimiter_lines_not_just_its_content() {
    // Lines 0..=3 are the opening fence, two content lines, closing fence. A
    // consumer painting a background behind the region needs all four; only
    // the middle two are content.
    let (_buf, regions) = markdown_regions("```rust\nlet x = 1;\nlet y = 2;\n```\n");

    assert_eq!(
        regions[0].rows,
        0..4,
        "rows must include both delimiter lines"
    );
}

#[test]
fn a_fence_inside_a_blockquote_leaves_the_container_prefix_in_the_gaps() {
    // The load-bearing property: `"> "` must never appear in the source a
    // parser is handed. Each content line's range starts AFTER that line's own
    // blockquote marker, so the marker bytes fall into the gap BETWEEN two
    // consecutive ranges and are dropped by construction.
    let (buf, regions) = markdown_regions("> ```rust\n> let x = 1;\n> let y = 2;\n> ```\n");

    assert_eq!(regions.len(), 1, "a fence nested in a blockquote is found");
    let region = &regions[0];
    assert_eq!(region.info, "rust");

    let text = reconstructed(&buf, region);
    assert_eq!(text, "let x = 1;\nlet y = 2;");
    assert!(
        !text.contains('>'),
        "the blockquote marker must never reach a parser as source: {text:?}"
    );

    // Stated structurally as well as by its effect: consecutive ranges are
    // genuinely discontiguous, and the bytes skipped are the container's.
    assert_eq!(region.content.len(), 2);
    let gap = region.content[0].end..region.content[1].start;
    assert!(
        gap.end > gap.start,
        "consecutive content lines must not be contiguous inside a container"
    );
    assert_eq!(buf.content().get(gap).unwrap_or(""), "\n> ");
}

#[test]
fn a_fence_inside_a_list_item_is_found_and_excludes_the_list_marker() {
    let (buf, regions) = markdown_regions("- item\n\n  ```rust\n  let x = 1;\n  ```\n");

    assert_eq!(regions.len(), 1, "a fence nested in a list item is found");
    let region = &regions[0];
    assert_eq!(region.info, "rust");
    assert_eq!(region.content.len(), 1);

    let text = reconstructed(&buf, region);
    assert!(
        !text.contains('-'),
        "the list marker must never reach a parser as source: {text:?}"
    );
    // A list item's CONTINUATION indent is retained, unlike a blockquote's
    // repeating `"> "` marker: the parser's per-line breakdown skips only
    // prefixes it can attribute to a repeating container marker, and a list
    // item's indent is not one. Pinned here as the behaviour that actually
    // holds so a future change to it is a deliberate, visible decision rather
    // than a silent drift.
    assert_eq!(text, "  let x = 1;");
}

#[test]
fn a_fence_with_no_info_string_still_produces_a_region() {
    // The region exists because the BYTES are code; whether a highlighter can
    // be found for them is a separate question, answered by each consumer.
    let (buf, regions) = markdown_regions("```\nsome code\n```\n");

    assert_eq!(
        regions.len(),
        1,
        "an untagged fence is still a region — it is skipped only by consumers \
         that need a language"
    );
    assert_eq!(regions[0].info, "");
    assert_eq!(reconstructed(&buf, &regions[0]), "some code");
}

#[test]
fn an_indented_code_block_is_a_region() {
    // Indented (non-fenced) code is code. It has no info string and no
    // delimiter lines, so every line it owns is content.
    let (buf, regions) = markdown_regions("paragraph\n\n    let x = 1;\n    let y = 2;\n");

    assert_eq!(regions.len(), 1, "an indented code block is a code region");
    let region = &regions[0];
    assert_eq!(region.info, "", "an indented block has no info string");
    assert_eq!(region.content.len(), 2);
    assert_eq!(
        region.rows,
        2..4,
        "rows cover exactly the lines the block occupies"
    );
    // The parser's per-line breakdown trusts the block's own start offset for
    // the FIRST line (comrak reports it past the 4-space indent) but falls
    // back to the physical line start for every continuation line, so the
    // indent survives on all lines but the first. Pinned as-is: it is
    // pre-existing behaviour of the shared per-line splitter, shared with
    // every other multi-line construct, and no consumer reads an empty-info
    // region's text today.
    assert_eq!(reconstructed(&buf, region), "let x = 1;\n    let y = 2;");
}

#[test]
fn a_table_is_not_a_code_region() {
    // Tables, HTML blocks and unrecognized nodes are verbatim passthroughs
    // like an indented code block, but they are emphatically NOT code — the
    // distinction the parser records so this collection can honour it.
    let (_buf, regions) = markdown_regions("| a | b |\n| - | - |\n| 1 | 2 |\n");

    assert!(
        regions.is_empty(),
        "a table must never be reported as code, got {} region(s)",
        regions.len()
    );
}

#[test]
fn an_html_block_is_not_a_code_region() {
    let (_buf, regions) = markdown_regions("<div>\n  hello\n</div>\n");

    assert!(
        regions.is_empty(),
        "an HTML block must never be reported as code, got {} region(s)",
        regions.len()
    );
}

#[test]
fn several_fences_are_returned_in_document_order() {
    let (buf, regions) = markdown_regions("```rust\nfirst\n```\n\n```python\nsecond\n```\n");

    assert_eq!(regions.len(), 2);
    assert_eq!(regions[0].info, "rust");
    assert_eq!(regions[1].info, "python");
    assert_eq!(reconstructed(&buf, &regions[0]), "first");
    assert_eq!(reconstructed(&buf, &regions[1]), "second");
    assert!(
        regions[0].rows.end <= regions[1].rows.start,
        "regions must be ordered by position"
    );
}

#[test]
fn a_code_document_is_exactly_one_region_covering_every_line() {
    // A code document is parsed by nobody — its block list is empty — so its
    // single region comes from the buffer's line structure instead. It is
    // otherwise an ordinary region: same shape, same per-line content.
    let content = "fn main() {\n    println!(\"hi\");\n}\n";
    let (buf, regions) = regions_of(content, DocumentKind::Code("rust"));

    assert_eq!(regions.len(), 1, "a code document is one whole region");
    let region = &regions[0];
    assert_eq!(
        region.info, "rust",
        "a code document's info is its detected language"
    );
    assert_eq!(
        region.content.len(),
        buf.line_count(),
        "one content range per buffer line — the region's lines are all of them"
    );
    assert_eq!(region.rows, 0..buf.line_count());
    assert_eq!(
        reconstructed(&buf, region),
        content,
        "the reconstruction of a whole code document is the document itself, \
         byte for byte"
    );
}

#[test]
fn a_plain_document_has_no_regions() {
    let (_buf, regions) = regions_of("just some text\nand more\n", DocumentKind::Plain);

    assert!(
        regions.is_empty(),
        "a Plain document has no language and no code"
    );
}

#[test]
fn an_image_document_has_no_regions() {
    let (_buf, regions) = regions_of("", DocumentKind::Image);

    assert!(regions.is_empty(), "an Image document has no code");
}

#[test]
fn a_markdown_document_with_no_code_has_no_regions() {
    let (_buf, regions) = markdown_regions("# heading\n\njust a paragraph.\n");

    assert!(regions.is_empty());
}
