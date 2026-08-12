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
    assert_eq!(text, "let x = 1;");
}

#[test]
fn a_fence_inside_a_list_item_inside_a_blockquote_excludes_both_prefixes() {
    let (buf, regions) = markdown_regions("> - item\n>\n>   ```rust\n>   let x = 1;\n>   ```\n");

    assert_eq!(
        regions.len(),
        1,
        "a fence nested in a list item inside a blockquote is found"
    );
    let region = &regions[0];
    assert_eq!(region.info, "rust");
    let text = reconstructed(&buf, region);
    assert!(
        !text.contains('>'),
        "the blockquote marker must never reach a parser as source: {text:?}"
    );
    assert!(
        !text.contains('-'),
        "the list marker must never reach a parser as source: {text:?}"
    );
    assert_eq!(text, "let x = 1;");
}

#[test]
fn a_fence_inside_a_list_item_nested_in_another_list_item_excludes_both_indents() {
    let (buf, regions) =
        markdown_regions("- outer\n  - inner\n\n    ```rust\n    let x = 1;\n    ```\n");

    assert_eq!(
        regions.len(),
        1,
        "a fence nested two list items deep is found"
    );
    let region = &regions[0];
    assert_eq!(region.info, "rust");
    let text = reconstructed(&buf, region);
    assert!(
        !text.contains('-'),
        "neither list item's marker may reach a parser as source: {text:?}"
    );
    assert_eq!(text, "let x = 1;");
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
    assert_eq!(reconstructed(&buf, region), "let x = 1;\nlet y = 2;");
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
    let rust = rune_syntax::LangId::from_name("rust").unwrap();
    let (buf, regions) = regions_of(content, DocumentKind::Code(rust));

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

#[test]
fn frontmatter_is_a_yaml_code_region() {
    // Frontmatter carries no info string a document could tag — its language
    // is implied by the `---` delimiter that opened it.
    let (buf, regions) = markdown_regions("---\ntitle: x\ndraft: true\n---\n\n# H\n");

    assert_eq!(
        regions.len(),
        1,
        "frontmatter must yield exactly one region"
    );
    let region = &regions[0];
    assert_eq!(region.info, "yaml");
    assert_eq!(
        region.content.len(),
        2,
        "one range per physical body line, delimiters excluded"
    );
    assert_eq!(reconstructed(&buf, region), "title: x\ndraft: true");
}

#[test]
fn frontmatter_rows_cover_its_delimiter_lines() {
    // Lines 0..=3 are the opening `---`, two body lines, the closing `---`.
    // A consumer painting a background behind the region needs all four.
    let (_buf, regions) = markdown_regions("---\ntitle: x\ndraft: true\n---\n\n# H\n");

    assert_eq!(
        regions[0].rows,
        0..4,
        "rows must include both delimiter lines"
    );
}

#[test]
fn blank_bodied_frontmatter_still_publishes_a_region() {
    // The one region deliberately published with nothing to highlight: its
    // delimiter lines are part of its rows, so dropping it would silently
    // erase the background from a document that visibly has frontmatter.
    let (buf, regions) = markdown_regions("---\n\n---\n");

    assert_eq!(
        regions.len(),
        1,
        "a blank body still leaves rows to paint a background over"
    );
    assert_eq!(regions[0].rows, 0..3);
    assert_eq!(reconstructed(&buf, &regions[0]), "");
}

#[test]
fn two_thematic_breaks_at_the_top_are_not_frontmatter() {
    // `---` immediately followed by `---` is two thematic breaks, never an
    // empty frontmatter block — the boundary of what counts as frontmatter
    // at all, decided by the parser rather than by the collection.
    let (_buf, regions) = markdown_regions("---\n---\n# H\n");

    assert!(
        regions.is_empty(),
        "two thematic breaks must never be reported as code, got {} region(s)",
        regions.len()
    );
}

#[test]
fn unterminated_frontmatter_produces_no_region() {
    let (_buf, regions) = markdown_regions("---\ntitle: x\n# H\n");

    assert!(
        regions.is_empty(),
        "an unclosed `---` is a thematic break plus text, not code"
    );
}

#[test]
fn a_thematic_break_mid_document_is_not_a_region() {
    let (_buf, regions) = markdown_regions("# H\n\n---\n\ntext\n");

    assert!(
        regions.is_empty(),
        "only a `---` opening the document can be frontmatter"
    );
}

#[test]
fn frontmatter_precedes_a_later_fence_in_document_order() {
    let (_buf, regions) = markdown_regions("---\na: 1\n---\n\n```rust\nfn f() {}\n```\n");

    assert_eq!(regions.len(), 2);
    assert_eq!(regions[0].info, "yaml");
    assert_eq!(regions[1].info, "rust");
    assert!(
        regions[0].rows.end <= regions[1].rows.start,
        "regions must be ordered by position"
    );
}
