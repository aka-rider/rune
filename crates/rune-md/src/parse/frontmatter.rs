//! Frontmatter's own parse: the delimiter that opens it, the language that
//! delimiter implies, and the split of a comrak `FrontMatter` node into its
//! two delimiter lines and the body between them.

use super::ScanHint;
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
/// Frontmatter, unlike a fence, has no unterminated shape to guard against:
/// comrak emits a `FrontMatter` node only once it has matched a closing
/// delimiter, and `is_valid_frontmatter_close` independently re-verifies
/// that the node's own last line IS that delimiter before the extension's
/// output is trusted at all. Only with BOTH of those holding does a last
/// line distinct from the first prove a closing delimiter line exists —
/// relax either and this starts claiming an arbitrary last line as a
/// delimiter.
///
/// The node's range stops at the last byte of the closing delimiter — the
/// newline after it belongs to no block — so every line strictly between
/// the two delimiters is body. A degenerate single-line node yields no
/// close, so its one line can never be claimed twice.
pub(super) fn build(
    content: &str,
    starts: &[usize],
    range: ByteRange,
    hint: &ScanHint,
) -> FrontmatterM {
    let first_line = super::line_at(starts, range.start);
    let last_line = super::last_line_of(starts, range);
    let lines = super::delimited::split(content, starts, range, hint, last_line > first_line);

    FrontmatterM {
        sm: RevealSm::new(RevealState::Revealed),
        range,
        first_line,
        last_line,
        open: lines.open,
        close: lines.close,
        content_lines: lines.content_lines,
    }
}
