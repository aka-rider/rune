// Single source of truth for the deterministic 5,000-line mixed markdown
// document used by both the criterion benchmark and the wall-clock perf
// guard test. Keeping one definition means the two can never silently
// diverge into measuring different documents.
//
// Included via `include!` rather than declared `pub` in a shared crate so
// neither target grows a dependency on the other's compilation unit.

/// Build a deterministic 5,000-line markdown document.
fn build_5k_doc() -> String {
    let mut doc = String::with_capacity(5_000 * 40);

    // Pattern: 36 lines per cycle (including a 4-line GFM table block:
    // header, delimiter, two body rows), repeated 138 times (4,968 lines)
    // + 32 extra lines = 5,000 total.
    let pattern: [&str; 36] = [
        "# Heading One",
        "## Heading Two",
        "### Heading Three",
        "#### Heading Four",
        "##### Heading Five",
        "###### Heading Six",
        "A paragraph with **bold**, *italic*, [a link](https://example.com), [[wikilink]], and `inline code`.",
        "```rust",
        "fn main() {",
        "    println!(\"hello\");",
        "}",
        "```",
        "> This is a blockquote line.",
        "- [ ] unchecked task",
        "\u{1f525} \u{65e5} \u{1f389} \u{4e2d}\u{6587} \u{d55c}\u{ad6d}\u{c5b4}",
        "",
        "1. ordered item alpha",
        "- unordered item beta",
        "---",
        "This is a continuation paragraph with **bold** and *italic* text.",
        "Another paragraph line with a [link](https://rust-lang.org) and `code`.",
        "> Blockquote continuation on a second line.",
        "Some ~~strikethrough~~ and **bold** inline formatting here.",
        "[Another link](https://doc.rust-lang.org) on its own line.",
        "[[target|label]] wikilink on its own line.",
        "```",
        "simple code block",
        "```",
        "- [x] completed task",
        "\u{1f680} \u{1f30d} \u{1f3af} CJK mixed content line",
        "",
        "| Name | Role | Notes |",
        "| :--- | :---: | ---: |",
        "| Alice | **Lead** | on the `api` team |",
        "| Bob | Reviewer | works on [docs](https://example.com/docs) |",
        "",
    ];

    let pattern_len = pattern.len(); // 36
    let full_cycles = 5_000 / pattern_len; // 138
    let remainder = 5_000 % pattern_len; // 32

    for _ in 0..full_cycles {
        for line in &pattern {
            doc.push_str(line);
            doc.push('\n');
        }
    }
    for line in &pattern[..remainder] {
        doc.push_str(line);
        doc.push('\n');
    }

    doc
}
