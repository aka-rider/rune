//! The single definition of "a region of code" every downstream consumer
//! reads — syntax highlighting and the background paint alike.
//!
//! Before this existed there were two unrelated notions: a whole
//! `DocumentKind::Code` document, highlighted through one pipeline, and a
//! fenced code block inside a markdown document, collected through a
//! completely separate one. They diverged in budgets, retention, coordinate
//! mapping and clamping, so identical code rendered differently depending on
//! which of the two it happened to be. `CodeRegion` collapses that: a whole
//! code document is simply a region whose lines happen to be all of them, and
//! a fence is a region whose lines happen to be discontiguous.

use std::ops::Range;

use crate::element::block::{Block, VerbatimKind};
use rune_core::buffer::Buffer;

/// One contiguous stretch of code, wherever it came from.
///
/// `content` is deliberately ONE range per physical line and must never be
/// collapsed into a single `first.start..last.end` span. When a fence sits
/// inside a container (a blockquote, a list item), that container's own
/// repeating prefix (`"> "`, a list marker's indent) lives in the GAP between
/// two consecutive lines' buffer ranges. A single contiguous range would
/// swallow those prefix bytes; the per-line decomposition drops them by
/// construction, and lets a consumer both reconstruct prefix-free source text
/// and map positions in that text back through the gaps to real buffer
/// offsets. Those prefix bytes must never reach a parser as source:
/// tree-sitter's error recovery silently absorbs a stray `"> "` for some
/// grammars but not for indentation-sensitive ones, which lose most of their
/// structure to it.
pub struct CodeRegion {
    /// A fence's info string, the detected language name for a `Code`
    /// document, or — for frontmatter — the language its opening delimiter
    /// implies, which is never read from the document at all. Empty means
    /// "nothing to highlight against" — the region still exists, because a
    /// consumer that paints a background cares only that the bytes are code,
    /// not whether a highlighter can be found for them.
    pub info: String,
    /// One buffer range per physical content line, container-prefix-free.
    pub content: Vec<Range<usize>>,
    /// The model-line span the region occupies, INCLUDING a fence's own
    /// delimiter lines. A consumer painting a background wants the delimiters
    /// covered too; when the fence is Rendered its delimiter lines are hidden
    /// and occupy no display rows at all, so no special case is needed to
    /// keep the two in step.
    pub rows: Range<usize>,
}

/// Every code region reachable from `blocks`, in document order.
///
/// Recurses into `Blockquote` children and `List` items — a fence can sit
/// inside either — and skips every other composite kind, none of which can
/// contain code. Indented (non-fenced) code blocks are collected alongside
/// fenced ones: the parser models them as `Block::Verbatim` with
/// `VerbatimKind::IndentedCode`, which is what distinguishes them from the
/// other verbatim passthroughs (tables, HTML, math, unrecognized nodes) that
/// are emphatically NOT code.
///
/// Frontmatter is the third kind, and the one that is published
/// unconditionally where a fence is not: its `---` delimiter lines are part
/// of its rows, so a frontmatter block whose body is blank still has rows a
/// consumer must paint a background over.
///
/// A region carrying an empty `info` is still emitted. For the fence and
/// indented-code kinds only, a region with no content lines at all is
/// dropped, since it describes no bytes.
pub(crate) fn collect(blocks: &[Block], buf: &Buffer, out: &mut Vec<CodeRegion>) {
    for block in blocks {
        match block {
            Block::CodeFence(cf) => {
                if cf.content_lines.is_empty() {
                    continue;
                }
                out.push(CodeRegion {
                    info: cf.language.clone(),
                    content: cf.content_lines.iter().map(|l| l.start..l.end).collect(),
                    // `first_line`/`last_line` are the whole fence including
                    // both delimiter lines, which is exactly what `rows` means.
                    rows: cf.first_line..cf.last_line.saturating_add(1),
                });
            }
            Block::Verbatim(v) if v.kind == VerbatimKind::IndentedCode => {
                let content: Vec<Range<usize>> =
                    v.content_lines.iter().map(|l| l.start..l.end).collect();
                // An indented code block has no delimiter lines and no info
                // string — every line it owns is content, so its row span is
                // derived from those lines rather than from stored fence
                // bounds. No content lines means no span, which is also the
                // one condition that drops the region.
                let Some(rows) = rows_of(&content, buf) else {
                    continue;
                };
                out.push(CodeRegion {
                    info: String::new(),
                    content,
                    rows,
                });
            }
            Block::Frontmatter(fm) => out.push(CodeRegion {
                info: crate::parse::frontmatter::LANGUAGE.to_string(),
                content: fm.content_lines.iter().map(|l| l.start..l.end).collect(),
                rows: fm.first_line..fm.last_line.saturating_add(1),
            }),
            Block::Blockquote(bq) => collect(&bq.children, buf, out),
            Block::List(list) => {
                for item in &list.items {
                    collect(&item.children, buf, out);
                }
            }
            _ => {}
        }
    }
}

/// The whole buffer as one region — what a `DocumentKind::Code` document is.
///
/// A code document parses to an EMPTY block list (the comrak parse is skipped
/// for every non-markdown kind), so this cannot be derived by walking blocks;
/// it comes from the buffer's own line structure instead. The buffer is
/// passed in rather than mirrored onto the machine: another owner writes it,
/// and a cached copy here would be a second source of truth for the same
/// value.
pub(crate) fn whole_document(info: &str, buf: &Buffer) -> CodeRegion {
    let content = (0..buf.line_count())
        .filter_map(|n| Some(buf.line_start(n)?..buf.line_end(n)?))
        .collect();
    CodeRegion {
        info: info.to_string(),
        content,
        rows: 0..buf.line_count(),
    }
}

/// The model-line span a set of per-line content ranges covers, or `None`
/// when there are none: no lines describe no rows, and row 0 is a real
/// answer that must never stand in for the absence of one. The caller
/// decides what that means — for `collect` it is the sole condition that
/// drops a region.
///
/// Reads each range's START: a line's end offset is the newline byte, which
/// belongs to that same line, so either end would do — the start is simply
/// the one that stays correct for a zero-length line range too.
fn rows_of(content: &[Range<usize>], buf: &Buffer) -> Option<Range<usize>> {
    let first = buf.offset_to_line_col(content.first()?.start).line;
    let last = buf.offset_to_line_col(content.last()?.start).line;
    Some(first..last.saturating_add(1))
}
