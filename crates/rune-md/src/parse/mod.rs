//! comrak invocation + AST walk -> `Block`/`Inline` tree (plan Context,
//! "Parse"). Sourcepos -> byte-range conversion is the WP0-proven formula
//! from `tests/spike_sourcepos.rs`, shared here so the spike itself now
//! calls this module instead of keeping its own copy (Ground rule 3: "reuse
//! that conversion"). `block.rs`/`inline.rs` hold the AST -> `Block`/
//! `Inline` construction; this file holds the primitives they're built on.

mod block;
mod blockquote;
mod delimited;
mod embed;
pub(crate) mod frontmatter;
mod inline;
mod table;

use crate::element::block::Block;
use comrak::nodes::{AstNode, Sourcepos};
use comrak::{Arena, Options, parse_document};
use rune_syntax::element::ByteRange;
use std::borrow::Cow;

/// Byte offset of the start of each BUFFER line: a line ends at `\n`,
/// nothing else. This is the ONLY line model this
/// crate ever needs: `cr_shadow` makes comrak's own CommonMark line count
/// agree with it by construction (see that function's docs), so `starts`
/// derived here serves the buffer, the emitter, `ScanHint`'s container-
/// prefix scanning, per-line marker/content clamping, AND every
/// sourcepos-to-byte-range conversion alike — there is no second, comrak-
/// only line model to keep in sync with this one.
pub fn line_starts(src: &str) -> Vec<usize> {
    let mut starts = vec![0usize];
    for (i, b) in src.bytes().enumerate() {
        if b == b'\n' {
            starts.push(i + 1);
        }
    }
    starts
}

/// A length-preserving PARSE-TIME view of `content`, fed to comrak
/// instead of the real buffer: every LONE `\r` (a CR NOT immediately
/// followed by `\n`) becomes a single space. CRLF is left alone — both
/// this crate's `\n`-only line model and CommonMark's own CR/LF/CRLF rule
/// already agree that a `"\r\n"` pair ends exactly one line, so nothing
/// there needs changing. A lone `\r` is where they used to disagree:
/// CommonMark (and so comrak's own `Sourcepos`) treats it as a line
/// terminator, but this crate's buffer model does not (a bare `\r` is
/// ordinary mid-line content, matching `rune_core::buffer::Buffer`).
/// Blanking it out here
/// makes that disagreement unrepresentable: comrak can no longer end a
/// line anywhere `line_starts` wouldn't, so every `Sourcepos` it hands
/// back converts through `line_starts` alone, correctly, always.
///
/// One byte replaced by one byte, so every offset comrak reports against
/// THIS view is already a valid offset into the REAL `content` — nothing
/// downstream needs to translate between two coordinate spaces. This is
/// strictly a view comrak parses: `parse()` is the only caller, the
/// replaced bytes are never written back to `content`, the buffer, or the
/// user's file (this changes what comrak SEES, never what the user WROTE).
/// `Cow::Borrowed` when no lone `\r` is
/// present — the overwhelming common case — so this costs nothing for an
/// ordinary document.
pub(crate) fn cr_shadow(content: &str) -> Cow<'_, str> {
    let bytes = content.as_bytes();
    let is_lone_cr = |i: usize, b: u8| b == b'\r' && bytes.get(i + 1) != Some(&b'\n');
    if !bytes.iter().enumerate().any(|(i, &b)| is_lone_cr(i, b)) {
        return Cow::Borrowed(content);
    }
    let mut shadow = bytes.to_vec();
    for (i, b) in shadow.iter_mut().enumerate() {
        if is_lone_cr(i, *b) {
            *b = b' ';
        }
    }
    // Every replaced byte is ASCII (`\r` -> `' '`), so this can never
    // turn valid UTF-8 into invalid UTF-8; the fallback is unreachable in
    // practice but keeps this function panic-free rather than relying on
    // that invariant unchecked.
    Cow::Owned(String::from_utf8(shadow).unwrap_or_else(|_| content.to_owned()))
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
    opts.extension.front_matter_delimiter = Some(frontmatter::DELIMITER.to_owned());
    opts.extension.autolink = true;
    opts
}

/// `options()` with the frontmatter extension turned off — the fallback
/// `parse()` uses for the one shape `frontmatter_extension_is_safe`
/// detects as untrustworthy.
fn options_without_frontmatter() -> Options<'static> {
    let mut opts = options();
    opts.extension.front_matter_delimiter = None;
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

/// The line index `i` such that `starts[i] <= offset < starts[i+1]`.
/// Shared with `emit/`.
pub(crate) fn line_at(starts: &[usize], offset: usize) -> usize {
    let idx = starts.partition_point(|&s| s <= offset);
    idx.saturating_sub(1)
}

/// The buffer line containing `range`'s last byte — a block's own final
/// physical line, by `starts`' `\n`-only counting rather than comrak's.
/// Derived from the last INCLUDED byte, never from `range.end` itself: an
/// exclusive end sitting exactly on a line start would otherwise report the
/// following, untouched line.
pub(crate) fn last_line_of(starts: &[usize], range: ByteRange) -> usize {
    line_at(starts, range.end.saturating_sub(1).max(range.start))
}

/// The ONLY place a raw comrak `Sourcepos` becomes an absolute byte
/// range — through `starts` (`line_starts(content)`, the SAME index the
/// buffer, the emitter, and `ScanHint` all use). Safe by construction:
/// `parse()` never hands comrak anything but its `cr_shadow` view, so
/// comrak's own line count and `starts` can never disagree.
pub(crate) fn node_range(content: &str, starts: &[usize], node: &AstNode) -> ByteRange {
    let sp = node.data.borrow().sourcepos;
    sourcepos_to_range(starts, sp).clamp(content.len())
}

/// One `ByteRange` per physical line `range` spans — the single
/// chokepoint EVERY multi-line construct's own per-line, `hint`-aware
/// breakdown routes through (`CodeFenceM`'s content lines, `HeadingM`'s
/// setext content lines, `VerbatimM`'s table/HTML-block/unknown content
/// lines, and a bare `TextRun`'s own content lines — verification round
/// 9's exhaustive audit: ANY node whose OWN raw extent can span more than
/// one physical line needs this, not just the block kinds already fixed
/// in rounds 4-7). Pushing a multi-line `range` whole through the generic
/// per-line splitter (`emit::for_each_line_slice`, which only knows
/// BUFFER line boundaries) re-claims a container's own repeating prefix
/// on any continuation line the range crosses — this derives each line's
/// OWN range instead, explicitly excluding that prefix via `hint`.
///
/// The first line trusts `range.start` (a node's own sourcepos-derived
/// first-line start is always reliable — see `CodeFenceM::fence_open`'s
/// docs); every CONTINUATION line uses `hint.start_for_line` to skip a
/// repeating container prefix comrak's sourcepos alone can't be trusted
/// to exclude. Naturally collapses to `vec![range]` for a genuinely
/// single-line node — the overwhelmingly common case, so this is cheap
/// for everything that doesn't need it.
pub(crate) fn per_line_content(
    content: &str,
    starts: &[usize],
    range: ByteRange,
    hint: &ScanHint,
) -> Vec<ByteRange> {
    let first_line = line_at(starts, range.start);
    let last_line = last_line_of(starts, range);
    (first_line..=last_line)
        .map(|l| {
            let s = if l == first_line {
                range.start
            } else {
                hint.start_for_line(starts, l)
            };
            let e = line_end_at(content.len(), starts, l).min(range.end).max(s);
            ByteRange::new(s, e).clamp(content.len())
        })
        .collect()
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

/// True unless comrak's frontmatter extension has desynced its OWN
/// internal line count on this specific document (verification round 5
/// CLASS A fallout, found by the widened fuzz alphabet — NOT part of the
/// reviewer's original CLASS A/B reports). Verified empirically: comrak's
/// frontmatter extension appears to search for its closing `"---"`
/// delimiter using `\n`-only line splitting internally, but then reports
/// `Sourcepos` through the OUTER, CR/LF/CRLF-aware line counter that the
/// REST of comrak's block parser keeps counting from afterward — the
/// same "one internal scan, a DIFFERENT reported line basis" shape as
/// round 4's wikilink-extension desync, but with a DOCUMENT-WIDE blast
/// radius here (frontmatter parsing runs first, so every later block's
/// sourcepos comes out wrong too) rather than one paragraph's siblings.
/// Detected by the one cheap, reliable signal available: a genuine
/// frontmatter block's own (correctly converted) range always ends on a
/// closing `"---"` line; if it doesn't, comrak's internal state for the
/// rest of this document can't be trusted at all. `parse()` reacts by
/// re-parsing the WHOLE document with the extension turned off — the
/// `"---...---"` blob degrades to ordinary paragraphs/thematic breaks
/// (unknown syntax degrades to visible raw text, never lost), which
/// this crate's other producers are already proven safe against.
fn frontmatter_extension_is_safe(content: &str, shadow: &str, starts: &[usize]) -> bool {
    let arena = Arena::new();
    let opts = options();
    let root = parse_document(&arena, shadow, &opts);
    match root.first_child() {
        Some(first)
            if matches!(
                first.data.borrow().value,
                comrak::nodes::NodeValue::FrontMatter(_)
            ) =>
        {
            let range = node_range(content, starts, first);
            frontmatter::is_valid_frontmatter_close(content, range)
        }
        _ => true,
    }
}

/// Parse `content` into the top-level `Block` tree. This is the ONLY entry
/// point `DocMachine::sync_content` calls — every downstream module reaches
/// comrak through here. Comrak itself only ever sees `shadow`
/// (`cr_shadow(content)`, a lone-`\r`-blanked view — see that function's
/// docs); every downstream `Block`/`Inline` still carries byte ranges into
/// the REAL `content`, since the view is length-preserving.
pub fn parse(content: &str) -> Vec<Block> {
    let shadow = cr_shadow(content);
    let starts = line_starts(content);
    let opts = if frontmatter_extension_is_safe(content, &shadow, &starts) {
        options()
    } else {
        options_without_frontmatter()
    };
    let arena = Arena::new();
    let root = parse_document(&arena, &shadow, &opts);
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
    use crate::element::block::Block;
    use crate::element::inline::{EmphasisKind, Inline};
    use rune_syntax::element::RevealState;

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
        assert_eq!(text_of(content, cf.fence_open), "```rust");
        assert_eq!(text_of(content, cf.fence_close.unwrap()), "```");
        // One range per content line (never one contiguous span — see
        // CodeFenceM's docs), each excluding its own trailing `\n` like
        // every other per-line range in this crate.
        assert_eq!(cf.content_lines.len(), 1);
        assert_eq!(text_of(content, cf.content_lines[0]), "fn f() {}");
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
}

#[cfg(test)]
mod image_tests;
