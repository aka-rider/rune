//! Frontmatter's own parse: the delimiter that opens it, the language that
//! delimiter implies, and the split of a comrak `FrontMatter` node into its
//! two delimiter lines and the body between them.

use super::ScanHint;
use crate::element::block::FrontmatterM;
use comrak::{Arena, parse_document};
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

/// Mirrors comrak's own opening check (`split_off_front_matter`): a leading
/// BOM is skipped, then the document must start with `DELIMITER`
/// immediately followed by `\n` or `\r\n`.
pub(super) fn shadow_may_open_frontmatter(shadow: &str) -> bool {
    let rest = shadow
        .strip_prefix('\u{feff}')
        .unwrap_or(shadow)
        .strip_prefix(DELIMITER);
    matches!(rest, Some(rest) if rest.starts_with('\n') || rest.starts_with("\r\n"))
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
///
/// Skips the reparse entirely — `shadow` cannot start a `FrontMatter` node
/// at all — for every document `shadow_may_open_frontmatter` rejects.
pub(super) fn frontmatter_extension_is_safe(content: &str, shadow: &str, starts: &[usize]) -> bool {
    if !shadow_may_open_frontmatter(shadow) {
        return true;
    }
    let arena = Arena::new();
    let opts = super::options();
    let root = parse_document(&arena, shadow, &opts);
    match root.first_child() {
        Some(first)
            if matches!(
                first.data.borrow().value,
                comrak::nodes::NodeValue::FrontMatter(_)
            ) =>
        {
            let range = super::node_range(content, starts, first);
            is_valid_frontmatter_close(content, range)
        }
        _ => true,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn shadow_may_open_frontmatter_matches_comrak_own_opening_check() {
        assert!(!shadow_may_open_frontmatter("body only\n"));
        assert!(!shadow_may_open_frontmatter("--\nnot enough dashes\n"));
        assert!(!shadow_may_open_frontmatter("---no newline after\n"));
        assert!(shadow_may_open_frontmatter("---\ntitle: x\n---\n"));
        assert!(shadow_may_open_frontmatter("---\r\ntitle: x\r\n---\r\n"));
        assert!(shadow_may_open_frontmatter("\u{feff}---\ntitle: x\n---\n"));
    }
}
