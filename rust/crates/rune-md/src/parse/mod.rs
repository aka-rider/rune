//! comrak invocation + AST walk -> `Block`/`Inline` tree (plan Context,
//! "Parse"). Sourcepos -> byte-range conversion is the WP0-proven formula
//! from `tests/spike_sourcepos.rs`, shared here so the spike itself now
//! calls this module instead of keeping its own copy (Ground rule 3: "reuse
//! that conversion"). `block.rs`/`inline.rs` hold the AST -> `Block`/
//! `Inline` construction; this file holds the primitives they're built on.

mod block;
mod inline;

use crate::element::ByteRange;
use crate::element::block::Block;
use comrak::nodes::{AstNode, Sourcepos};
use comrak::{Arena, Options, parse_document};

/// Byte offset of the start of each line — port of
/// `pkg/editor/buffer/lineindex.go:computeLineStarts`, and the index every
/// sourcepos conversion below is built on.
pub fn line_starts(src: &str) -> Vec<usize> {
    let mut starts = vec![0usize];
    for (i, b) in src.bytes().enumerate() {
        if b == b'\n' {
            starts.push(i + 1);
        }
    }
    starts
}

/// The WP0-proven conversion: comrak `Sourcepos` (1-based, end-inclusive,
/// UTF-8 byte columns) -> an absolute half-open `ByteRange`.
/// `start = line_starts[l-1] + (c-1)`, `end = line_starts[el-1] + ec`
/// (Gotchas: "comrak sourcepos is 1-based, end-inclusive, byte columns").
/// Never panics on a malformed/out-of-range sourcepos: every lookup goes
/// through `.get()` and falls back to 0 rather than indexing directly.
pub fn sourcepos_to_range(starts: &[usize], sp: Sourcepos) -> ByteRange {
    let start_line = starts
        .get(sp.start.line.saturating_sub(1))
        .copied()
        .unwrap_or(0);
    let end_line = starts
        .get(sp.end.line.saturating_sub(1))
        .copied()
        .unwrap_or(0);
    let start = start_line + sp.start.column.saturating_sub(1);
    let end = end_line + sp.end.column;
    ByteRange::new(start, end)
}

/// The one comrak `Options` value the whole crate parses with — extension
/// set from plan Context ("Parse"). Sourcepos needs no option (Gotchas:
/// "There is no option that enables AST sourcepos").
pub fn options() -> Options<'static> {
    let mut opts = Options::default();
    opts.extension.strikethrough = true;
    opts.extension.tasklist = true;
    opts.extension.table = true;
    opts.extension.wikilinks_title_after_pipe = true;
    opts.extension.front_matter_delimiter = Some("---".to_owned());
    opts
}

pub(crate) fn line_start_at(starts: &[usize], line: usize) -> usize {
    starts.get(line).copied().unwrap_or(0)
}

/// The byte offset of the end of `line`, exclusive of its own trailing
/// `\n` — mirrors `rune_core::buffer::Buffer::line_end`. Shared with
/// `emit/`, which splits spans across the same line boundaries.
pub(crate) fn line_end_at(content_len: usize, starts: &[usize], line: usize) -> usize {
    let count = starts.len();
    if line + 1 >= count {
        return content_len;
    }
    starts
        .get(line + 1)
        .copied()
        .unwrap_or(content_len)
        .saturating_sub(1)
        .min(content_len)
}

/// The line index `i` such that `starts[i] <= offset < starts[i+1]` — port
/// of `pkg/editor/buffer/lineindex.go`'s `findLine`. Shared with `emit/`.
pub(crate) fn line_at(starts: &[usize], offset: usize) -> usize {
    let idx = starts.partition_point(|&s| s <= offset);
    idx.saturating_sub(1)
}

pub(crate) fn node_range(content: &str, starts: &[usize], node: &AstNode) -> ByteRange {
    let sp = node.data.borrow().sourcepos;
    sourcepos_to_range(starts, sp).clamp(content.len())
}

/// Per-line scan-start hints for locating a blockquote depth's own `"> "`
/// marker. Only line 0 of a nested `BlockQuote` node's OWN sourcepos tells
/// us where that depth's content starts; for a CONTINUATION line (line >
/// first_line) within a multi-line blockquote, comrak gives no such
/// per-line signal, so scanning from the physical line start would find
/// the OUTERMOST `'>'` regardless of depth — every nested depth would
/// report the identical marker range on that line, double-hiding the same
/// bytes and leaving the inner depths' actual `"> "` unmodeled (MAJOR 4,
/// the multi-line-continuation half of the bug: `"> > nested\n> > nested"`
/// — line 1 needs the SAME depth-aware treatment line 0 gets from
/// `range.start`). `ScanHint` threads that knowledge down the recursion:
/// each nested depth's hint is built from its own immediately enclosing
/// depth's just-computed markers (mapping line -> marker end), falling
/// back to the enclosing depth's own hint on a line where the enclosing
/// depth found no marker (a lazy continuation line).
pub(crate) enum ScanHint<'p> {
    /// The document root: every line scans from its own physical start.
    Root,
    Nested {
        marker_ends: std::collections::HashMap<usize, usize>,
        parent: &'p ScanHint<'p>,
    },
}

impl ScanHint<'_> {
    pub(crate) fn start_for_line(&self, starts: &[usize], line: usize) -> usize {
        match self {
            ScanHint::Root => line_start_at(starts, line),
            ScanHint::Nested {
                marker_ends,
                parent,
            } => marker_ends
                .get(&line)
                .copied()
                .unwrap_or_else(|| parent.start_for_line(starts, line)),
        }
    }
}

/// Parse `content` into the top-level `Block` tree. This is the ONLY entry
/// point `DocMachine::sync_content` calls — every downstream module reaches
/// comrak through here.
pub fn parse(content: &str) -> Vec<Block> {
    let starts = line_starts(content);
    let arena = Arena::new();
    let opts = options();
    let root = parse_document(&arena, content, &opts);
    block::build_blocks(content, &starts, root, &ScanHint::Root)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use super::*;
    use crate::element::RevealState;
    use crate::element::block::Block;
    use crate::element::inline::{EmphasisKind, Inline};

    fn text_of(content: &str, r: ByteRange) -> &str {
        content.get(r.start..r.end).unwrap()
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
        assert_eq!(text_of(content, cf.fence_open.unwrap()), "```rust");
        assert_eq!(text_of(content, cf.fence_close.unwrap()), "```");
        // `content` runs up to the start of the closing fence LINE, so it
        // keeps the content line's own trailing `\n` verbatim (§1.4.5); the
        // emitter (`emit/`) is what clips per-line at render time.
        assert_eq!(text_of(content, cf.content), "fn f() {}\n");
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
        let content = "| a | b |\n|---|---|\n| 1 | 2 |\n";
        let blocks = parse(content);
        assert!(matches!(blocks[0], Block::Verbatim(_)));
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
    fn inline_image_is_plain_text_run() {
        let content = "![alt](img.png)\n";
        let blocks = parse(content);
        let Block::Paragraph(p) = &blocks[0] else {
            panic!("expected paragraph");
        };
        assert!(matches!(p.inlines[0], Inline::Text(_)));
        let Inline::Text(t) = &p.inlines[0] else {
            unreachable!()
        };
        assert_eq!(text_of(content, t.range), "![alt](img.png)");
    }
}
