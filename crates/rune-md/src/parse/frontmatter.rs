//! Frontmatter's own parse: the delimiter that opens it, the language that
//! delimiter implies, and the split of a comrak `FrontMatter` node into its
//! two delimiter lines and the body between them.

use super::{ScanHint, line_end_at};
use crate::element::block::FrontmatterM;
use rune_syntax::element::{ByteRange, RevealSm, RevealState};

/// The one spelling of the frontmatter delimiter. comrak is configured with
/// it and the closing-line check below compares against it — a second
/// literal would let the two drift apart silently.
pub(crate) const DELIMITER: &str = "---";

/// The language a `DELIMITER`-opened block is written in. Implied by the
/// delimiter alone: frontmatter carries no info string a document could
/// tag, so this is never read from the document.
pub(crate) const LANGUAGE: &str = "yaml";

/// True if `range`'s own last line — as comrak (via our conversion)
/// reports it — is genuinely a closing `DELIMITER` line: the sanity check
/// `frontmatter_extension_is_safe` uses to decide whether a `FrontMatter`
/// node's reported range can be trusted at all (see that function's docs
/// for the comrak-internal desync it exists to detect).
pub(super) fn is_valid_frontmatter_close(content: &str, range: ByteRange) -> bool {
    let Some(text) = content.get(range.start..range.end) else {
        return false;
    };
    let trimmed = text.strip_suffix('\n').unwrap_or(text);
    trimmed.rsplit('\n').next() == Some(DELIMITER)
}

/// Split a `FrontMatter` node into its opening delimiter line, its body
/// lines, and its closing delimiter line.
///
/// The node's range stops at the last byte of the closing delimiter — the
/// newline after it belongs to no block — so `range`'s own last line IS the
/// closing delimiter line, and every line strictly between the two
/// delimiters is body. `close` exists only when those two lines are
/// distinct, so a degenerate single-line node can never have its one line
/// claimed twice.
///
/// Body lines are one range each, never one contiguous span: a collapsed
/// range would swallow the `\n` bytes that separate them, which a consumer
/// reconstructing the body supplies itself.
pub(super) fn build(
    content: &str,
    starts: &[usize],
    range: ByteRange,
    hint: &ScanHint,
) -> FrontmatterM {
    let len = content.len();
    let first_line = super::line_at(starts, range.start);
    let last_line = super::line_at(starts, range.end.saturating_sub(1).max(range.start));

    // The first line starts at `range.start`, every later line at its own
    // `hint`-derived content start — the same split `parse::block`'s fenced
    // arm makes, and for the same reason: a node's sourcepos already bakes
    // in any container prefix on its first line but nowhere else.
    let open = Some(ByteRange::new(range.start, line_end_at(len, starts, first_line)).clamp(len));
    let close = (last_line > first_line).then(|| {
        ByteRange::new(
            hint.start_for_line(starts, last_line),
            line_end_at(len, starts, last_line),
        )
        .clamp(len)
    });
    let content_lines = ((first_line + 1)..last_line)
        .map(|l| {
            ByteRange::new(hint.start_for_line(starts, l), line_end_at(len, starts, l)).clamp(len)
        })
        .collect();

    FrontmatterM {
        sm: RevealSm::new(RevealState::Revealed),
        range,
        first_line,
        last_line,
        open,
        close,
        content_lines,
    }
}
