//! comrak invocation + AST walk -> `Block`/`Inline` tree (plan Context,
//! "Parse"). Sourcepos -> byte-range conversion is the WP0-proven formula
//! from `tests/spike_sourcepos.rs`, shared here so the spike itself now
//! calls this module instead of keeping its own copy (Ground rule 3: "reuse
//! that conversion"). `block.rs`/`inline.rs` hold the AST -> `Block`/
//! `Inline` construction; this file holds the primitives they're built on.
//!
//! Comrak-quirk workarounds, one line each — the function compensating for
//! the quirk, not merely documenting it:
//! - frontmatter's own line count desyncing the rest of the document's
//!   sourcepos: `frontmatter_extension_is_safe`, `frontmatter.rs`.
//! - a WikiLink embedding a raw newline desyncing comrak's line counter for
//!   the rest of a paragraph: `build_inlines`, `inline.rs`.
//! - a multi-line code span's reported end column landing on a neighbouring
//!   line: `trailing_backtick_run`, `inline.rs`.
//! - a thematic break's sourcepos overrunning an empty blockquote
//!   continuation line: `build_thematic_break`, `block.rs`.
//! - a residual inline `Text` node left over a setext underline:
//!   `underline_of_setext_heading`, `block.rs`.
//! - comrak's own `within_brackets` guard suppressing `![[...]]` as a
//!   WikiLink node: `wikilink_role`, `catalogue.rs`.

mod block;
mod blockquote;
mod delimited;
mod embed;
pub(crate) mod frontmatter;
mod indent;
mod inline;
mod shadow;
mod table;

use crate::element::block::Block;
use comrak::nodes::{AstNode, Sourcepos};
use comrak::{Arena, Options, parse_document};
use rune_syntax::element::ByteRange;
use shadow::Round;

pub use shadow::parse_shadow;

/// Byte offset of the start of each BUFFER line: a line ends at `\n`,
/// nothing else. This is the ONLY line model this
/// crate ever needs: `parse_shadow` makes comrak's own CommonMark line
/// count agree with it by construction (see that function's docs), so
/// `starts` derived here serves the buffer, the emitter, `ScanHint`'s
/// container-prefix scanning, per-line marker/content clamping, AND every
/// sourcepos-to-byte-range conversion alike — there is no second, comrak-
/// only line model to keep in sync with this one.
pub fn line_starts(src: &str) -> Vec<usize> {
    rune_core::buffer::line_starts(src)
}

/// comrak `Sourcepos` (1-based, end-inclusive, UTF-8 byte columns) -> an
/// absolute half-open `ByteRange` into `content`, always in bounds and
/// always on char boundaries. `sp` must come from a parse of
/// `parse_shadow(content)`: columns are offsets into THAT copy, and only
/// its coordinates are byte-exact. A column still never leaves the line
/// it names: comrak reports a multi-line code span's end against the
/// indentation of an earlier line, which can point past the end of the
/// last one.
///
/// One shape stays outside this: comrak can still consume part of a tab
/// that pads a LIST MARKER, which is not container prefix and cannot be
/// expanded without changing the padding comrak matches list items on.
/// That line becomes an indented code block, which has no inline children
/// and whose own bounds comrak takes from byte counters rather than from
/// the shifted content. `tests/spike_sourcepos.rs` pins it.
pub fn sourcepos_to_range(content: &str, starts: &[usize], sp: Sourcepos) -> ByteRange {
    let start = offset_of_column(
        content,
        starts,
        sp.start.line,
        sp.start.column.saturating_sub(1),
        Round::Down,
    );
    let end = offset_of_column(content, starts, sp.end.line, sp.end.column, Round::Up);
    let start = content.floor_char_boundary(start.min(content.len()));
    let end = content
        .ceil_char_boundary(end.min(content.len()))
        .max(start);
    ByteRange::new(start, end)
}

fn offset_of_column(
    content: &str,
    starts: &[usize],
    line: usize,
    columns: usize,
    round: Round,
) -> usize {
    let index = line.saturating_sub(1);
    let line_start = starts.get(index).copied().unwrap_or(0);
    let line_limit = starts.get(line).copied().unwrap_or(content.len());
    let line_bytes = content
        .as_bytes()
        .get(line_start..line_limit)
        .unwrap_or_default();
    let offset = line_start + shadow::real_offset_in_line(line_bytes, index == 0, columns, round);
    offset.min(line_limit)
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
/// `parse()` never hands comrak anything but its `parse_shadow` copy, so
/// comrak's own line count and `starts` can never disagree.
pub(crate) fn node_range(content: &str, starts: &[usize], node: &AstNode) -> ByteRange {
    let sp = node.data.borrow().sourcepos;
    sourcepos_to_range(content, starts, sp)
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
            let line_end = line_end_at(content.len(), starts, l);
            let s = if l == first_line {
                range.start
            } else {
                hint.start_for_line(starts, l)
            }
            .min(line_end);
            let e = line_end.min(range.end).max(s);
            ByteRange::new(s, e).clamp(content.len())
        })
        .collect()
}

/// Per-line scan-start hints for locating a container's own claim on a
/// content line: a blockquote depth's `"> "`, re-scanned fresh per line,
/// or a list item's/indented code block's own fixed continuation width,
/// computed once from its first line and held constant. Both populate the same
/// `marker_ends` map — line -> the byte offset right after whatever this
/// depth claims on it — because both need the identical fallback: a line
/// this depth found no claim on (a lazy continuation line, e.g. `"> >
/// nested\n> > nested"`'s line 1 for the inner depth, or an under-indented
/// paragraph continuation inside a list item) defers to `parent`, the
/// immediately enclosing depth's own hint, rather than the raw physical
/// line start. Nesting composes by construction: each depth's hint is
/// built with `parent` set to the depth it sits inside, so a fence inside
/// a list item inside a blockquote strips both prefixes without either
/// depth needing to know about the other.
pub(crate) enum ScanHint<'p> {
    /// The document root: every line scans from its own physical start.
    Root,
    Nested {
        marker_ends: std::collections::HashMap<usize, usize>,
        /// Whether THIS depth's own claim already has an independent
        /// concealment claim elsewhere (a blockquote's `BlockquoteMarkerM`,
        /// hidden per line in its own right) rather than depending on a
        /// descendant's range to cover it (a list item's/indented code
        /// block's fixed width, which has no such independent claim —
        /// nothing hides it if a descendant's own range stops excluding
        /// it). `concealment_baseline` reads this to find the nearest
        /// depth whose prefix is safe to leave uncovered.
        conceals_own_prefix: bool,
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
                ..
            } => marker_ends
                .get(&line)
                .copied()
                .unwrap_or_else(|| parent.start_for_line(starts, line)),
        }
    }

    /// The earliest byte on `line` that is either genuinely unclaimed by
    /// any container, or claimed by a depth with its OWN independent
    /// concealment (so hiding stops there rather than double-claiming
    /// it). A descendant whose own concealment needs to hide everything
    /// back to the nearest such boundary — a setext heading's underline
    /// row, say — uses this instead of `start_for_line` (which composes
    /// every depth's claim, INCLUDING ones nothing else hides on its
    /// behalf).
    pub(crate) fn concealment_baseline(&self, starts: &[usize], line: usize) -> usize {
        match self {
            ScanHint::Root => line_start_at(starts, line),
            ScanHint::Nested {
                marker_ends,
                conceals_own_prefix: true,
                parent,
            } => marker_ends
                .get(&line)
                .copied()
                .unwrap_or_else(|| parent.concealment_baseline(starts, line)),
            ScanHint::Nested {
                conceals_own_prefix: false,
                parent,
                ..
            } => parent.concealment_baseline(starts, line),
        }
    }
}

/// Parse `content` into the top-level `Block` tree. This is the ONLY entry
/// point `DocMachine::sync_content` calls — every downstream module reaches
/// comrak through here. Comrak itself only ever sees `shadow`
/// (`parse_shadow(content)` — see that function's docs); every downstream
/// `Block`/`Inline` still carries byte ranges into the REAL `content`,
/// which `sourcepos_to_range` translates back to.
pub fn parse(content: &str) -> Vec<Block> {
    let shadow = parse_shadow(content);
    let starts = line_starts(content);
    let opts = if frontmatter::frontmatter_extension_is_safe(content, &shadow, &starts) {
        options()
    } else {
        options_without_frontmatter()
    };
    let arena = Arena::new();
    let root = parse_document(&arena, &shadow, &opts);
    block::build_blocks(content, &starts, root, &ScanHint::Root, 0)
}

#[cfg(test)]
#[path = "parse_tests.rs"]
mod tests;

#[cfg(test)]
mod image_tests;
