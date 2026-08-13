//! What frontmatter emits, stated as a specification.
//!
//! Frontmatter is published as a code region, so its emit is split in two:
//! the `---` delimiter lines keep the dim `comment` tone, and the body
//! between them is emitted as code, the same as a fence's content. These
//! tests pin that split, pin that the body's bytes still round-trip
//! verbatim, and pin that the split claims exactly the same bytes the single
//! whole-range push it replaced did.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod conceal_common;

use conceal_common::{joined_line, synced};
use rune_md::emit::emit;
use rune_md::invariant::assert_no_duplicate_content;
use rune_syntax::SyntaxLine;
use rune_syntax::scope::ScopeId;

fn emitted(content: &str, focused: bool) -> Vec<SyntaxLine> {
    let (buf, doc) = synced(content, 0, focused);
    let (lines, _snap) = emit(buf.content(), doc.blocks(), 80);
    lines
}

fn scopes_on_line(lines: &[SyntaxLine], line: usize) -> Vec<ScopeId> {
    lines[line].spans.iter().map(|s| s.scope()).collect()
}

#[test]
fn frontmatter_delimiters_and_body_carry_different_scopes() {
    let table = rune_syntax::scope::scope_table();
    let delimiter = table.resolve("comment").expect("`comment` is registered");
    let body = table
        .resolve("markup.raw.block")
        .expect("`markup.raw.block` is registered");
    assert_ne!(
        delimiter, body,
        "the split is only meaningful if the two scopes differ"
    );

    let lines = emitted("---\ntitle: x\n---\n\n# H\n", true);

    for line in [0, 2] {
        let scopes = scopes_on_line(&lines, line);
        assert!(
            !scopes.is_empty(),
            "line {line} must emit at least one span"
        );
        assert!(
            scopes.iter().all(|&s| s == delimiter),
            "line {line} is a `---` delimiter and must be emitted entirely at \
             the delimiter scope, got {scopes:?}"
        );
    }

    let scopes = scopes_on_line(&lines, 1);
    assert!(
        !scopes.is_empty(),
        "the body line must emit at least one span"
    );
    assert!(
        scopes.iter().all(|&s| s == body),
        "the body line is code and must be emitted entirely at the code scope, \
         got {scopes:?}"
    );
}

#[test]
fn frontmatter_body_text_is_unchanged() {
    let content = "---\ntitle: x\ndraft: true\n---\n";

    for &focused in &[true, false] {
        let lines = emitted(content, focused);

        assert_eq!(
            joined_line(&lines, 1, content),
            "title: x",
            "the body must render byte for byte (focused={focused})"
        );
        assert_eq!(
            joined_line(&lines, 2, content),
            "draft: true",
            "the body must render byte for byte (focused={focused})"
        );
        assert_eq!(
            joined_line(&lines, 0, content),
            "---",
            "a delimiter is never concealed (focused={focused})"
        );
        assert_eq!(
            joined_line(&lines, 3, content),
            "---",
            "a delimiter is never concealed (focused={focused})"
        );
    }
}

#[test]
fn frontmatter_survives_the_coverage_invariants() {
    // Splitting one whole-range push into delimiters plus a per-line body
    // must claim exactly the same bytes, once each — including for a blank
    // body, for a document ending at EOF without a trailing newline,
    // alongside other block kinds, and for CRLF line endings.
    for content in [
        "---\ntitle: x\n---\n",
        "---\na: 1\nb: 2\n---\n# H\n",
        "---\n\n---\n",
        "---\nx\n---",
        "---\ntitle: x\n---\n\n> quoted\n\n```rust\nfn f() {}\n```\n",
        "---\r\ntitle: x\r\n---\r\n# H\r\n",
    ] {
        assert_no_duplicate_content(content);
    }
}

#[test]
fn a_revealed_fence_at_buffer_line_zero_survives_the_coverage_invariants() {
    // The shape frontmatter's emit copies: a Revealed block whose opening
    // delimiter, content lines and closing delimiter are three separate
    // per-line claims starting at buffer line 0.
    assert_no_duplicate_content("```rust\nfn f() {}\n```\n");
}
