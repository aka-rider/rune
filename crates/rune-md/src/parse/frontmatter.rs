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
}
