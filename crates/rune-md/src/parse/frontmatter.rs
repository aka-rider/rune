use super::ScanHint;
use crate::element::block::FrontmatterM;
use comrak::{Arena, parse_document};
use rune_syntax::element::{ByteRange, RevealSm, RevealState};

pub(crate) const DELIMITER: &str = "---";

pub(crate) const LANGUAGE: &str = "yaml";

pub(super) fn is_valid_frontmatter_close(content: &str, range: ByteRange) -> bool {
    let Some(text) = content.get(range.start..range.end) else {
        return false;
    };
    let trimmed = text.strip_suffix('\n').unwrap_or(text);
    trimmed.rsplit('\n').next() == Some(DELIMITER)
}

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

// Mirrors comrak's own opening check (`split_off_front_matter`): a leading
// BOM is skipped, then the document must start with `DELIMITER` immediately
// followed by `\n` or `\r\n`.
pub(super) fn shadow_may_open_frontmatter(shadow: &str) -> bool {
    let rest = shadow
        .strip_prefix('\u{feff}')
        .unwrap_or(shadow)
        .strip_prefix(DELIMITER);
    matches!(rest, Some(rest) if rest.starts_with('\n') || rest.starts_with("\r\n"))
}

// comrak's frontmatter extension searches for its closing "---" using
// `\n`-only line splitting internally, but reports `Sourcepos` through the
// outer CR/LF/CRLF-aware line counter the rest of the block parser uses —
// on a document with `\r\n`, every sourcepos after the frontmatter block
// comes out wrong. Detected by checking that the FrontMatter node's own
// range ends on a closing "---" line; if it doesn't, the whole document is
// re-parsed with the extension disabled.
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

    #[test]
    fn is_valid_frontmatter_close_requires_the_literal_delimiter_on_the_last_line() {
        let closed = "---\ntitle: x\n---\n";
        assert!(is_valid_frontmatter_close(
            closed,
            ByteRange::new(0, closed.len())
        ));
        let mismatched = "---\ntitle: x\n----\n";
        assert!(!is_valid_frontmatter_close(
            mismatched,
            ByteRange::new(0, mismatched.len())
        ));
    }

    /// A genuine `FrontmatterM` always spans an opening delimiter line AND a
    /// separate closing one (`is_valid_frontmatter_close`'s own contract on
    /// the node `build` is only ever called for — see `parse()`'s docs), so
    /// `last_line > first_line` never actually meets on real input. Pinned
    /// directly against a hand-built single-line range instead, the same
    /// way `catalogue`'s `heading_name` test pins a branch no real
    /// `parse()` input can reach: with only one line, there is no separate
    /// close line to report.
    #[test]
    fn a_single_line_range_reports_no_close_line() {
        let content = "---\n";
        let starts = crate::parse::line_starts(content);
        let fm = build(
            content,
            &starts,
            ByteRange::new(0, content.len()),
            &ScanHint::Root,
        );
        assert!(fm.close.is_none());
    }

    /// `frontmatter_extension_is_safe`'s guard only runs
    /// `is_valid_frontmatter_close` when comrak's own reparse genuinely
    /// produced a `FrontMatter` first child; otherwise it trusts the
    /// fallback `_ => true` unconditionally. A document that OPENS like
    /// frontmatter but never closes it (so the real first child degrades to
    /// something else — here a `ThematicBreak`) is the reachable case that
    /// pins the guard actually gating on node KIND: a leading BOM makes
    /// this crate's OWN `node_range` conversion degenerate to an EMPTY
    /// range for that non-FrontMatter fallback node (a comrak/BOM
    /// sourcepos quirk unrelated to frontmatter itself), and an empty range
    /// never satisfies `is_valid_frontmatter_close` — so a guard that ran
    /// the check against this node regardless of its kind would wrongly
    /// report `false` here.
    #[test]
    fn a_bom_prefixed_unterminated_frontmatter_opener_is_still_reported_safe() {
        let content = "\u{feff}---\nnot: closed\n";
        let shadow = crate::parse::parse_shadow(content);
        let starts = crate::parse::line_starts(content);
        assert!(frontmatter_extension_is_safe(content, &shadow, &starts));
    }
}
