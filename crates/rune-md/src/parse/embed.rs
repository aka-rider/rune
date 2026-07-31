//! Image range derivation and `![[target]]` embed recovery — split out of
//! `parse::inline` to keep that module under CONSTITUTION §1.6's 500-LoC
//! limit. `build_inline`'s `InlineKind::Image` arm calls
//! [`image_alt_range`]/[`image_target_range`] directly; `build_inlines`
//! funnels its own output through [`recover_embeds`] before returning, on
//! both its normal fall-through and its multiline-wikilink-recovery early
//! return.

use super::node_range;
use crate::element::inline::{ImageM, Inline, TextRun};
use comrak::nodes::AstNode;
use rune_syntax::element::{ByteRange, RevealSm, RevealState};

/// The alt-text range of a `![alt](url)` image node: the span from its
/// first child's start to its last child's end — the same derivation
/// `child_gap_delims` (`parse::inline`) uses for an emphasis/strong node's
/// inner content, just collapsed to the one inner span itself, since an
/// image has no separate open/close delimiter machinery of its own
/// ([`image_target_range`] below anchors off this same end). A childless
/// image (`![](url)`) has no content to bound a range against, so `alt`
/// collapses to an empty range positioned right after the `"!["` opener.
pub(super) fn image_alt_range(
    content: &str,
    starts: &[usize],
    node: &AstNode,
    range: ByteRange,
) -> ByteRange {
    match (node.first_child(), node.last_child()) {
        (Some(first), Some(last)) => {
            let start = node_range(content, starts, first)
                .start
                .max(range.start)
                .min(range.end);
            let end = node_range(content, starts, last)
                .end
                .max(range.start)
                .min(range.end);
            ByteRange::new(start, end)
        }
        _ => {
            let start = range.start.saturating_add(2).min(range.end);
            ByteRange::new(start, start)
        }
    }
}

/// `link_url_range`'s (`parse::inline`) image counterpart: an image is
/// always `"![" alt "](" url [" \"title\""] ")"`, so the URL begins exactly
/// 2 bytes after the alt text closes. `alt_end` is [`image_alt_range`]'s
/// own `end` — already correctly anchored for BOTH a populated and an empty
/// alt (an empty alt's closing position sits right after `"!["`, 2 bytes
/// further in than a link's own empty-text anchor), so this needs no
/// separate empty-alt fallback the way `link_url_range` does for an empty
/// link text.
pub(super) fn image_target_range(
    alt_end: usize,
    range: ByteRange,
    url: &str,
    content_len: usize,
) -> ByteRange {
    let url_start = alt_end.saturating_add(2).min(range.end).min(content_len);
    let url_end = url_start
        .saturating_add(url.len())
        .min(range.end)
        .min(content_len);
    ByteRange::new(url_start, url_end)
}

/// Recovers `![[target]]` embeds from the flattened `Text` runs
/// `build_inlines` hands back: comrak's own wikilink trigger has a
/// `within_brackets` guard that suppresses the node entirely under a
/// leading `!` (empirically verified, pinned in `catalogue.rs`'s
/// `embed_prefixed_wikilink_comrak_behaviour_is_pinned`), so `![[note]]`
/// parses as plain text with no dedicated AST node at all. Runs as a
/// post-pass over `build_inlines`'s own output — both its normal
/// fall-through and its multiline-wikilink-recovery early return funnel
/// through here — so every caller (paragraph, heading, list item, table
/// cell, and any container that recurses into `build_inlines`) gets embed
/// recovery uniformly, without duplicating the scan at each call site.
pub(super) fn recover_embeds(content: &str, starts: &[usize], inlines: Vec<Inline>) -> Vec<Inline> {
    let mut out = Vec::with_capacity(inlines.len());
    for inl in inlines {
        match inl {
            Inline::Text(t) => split_text_run_embeds(content, starts, t, &mut out),
            other => out.push(other),
        }
    }
    out
}

/// Splits one flattened `Text` run into alternating `Text`/`Image` pieces
/// wherever a `![[target]]` embed appears — scanned per `content_lines`
/// PIECE (never across the whole, possibly multi-line, `range`), so an
/// embed whose own `![[`/`]]` delimiters would straddle a raw newline is
/// deliberately left as plain text instead of reconstructed speculatively —
/// the same conservative posture `build_inlines`'s own multiline-wikilink
/// recovery takes for a genuine `WikiLink` node.
fn split_text_run_embeds(content: &str, starts: &[usize], t: TextRun, out: &mut Vec<Inline>) {
    for line_range in &t.content_lines {
        let Some(line_text) = content.get(line_range.start..line_range.end) else {
            out.push(Inline::Text(TextRun {
                range: *line_range,
                content_lines: vec![*line_range],
            }));
            continue;
        };
        let embeds = find_embeds_in_line(line_text);
        if embeds.is_empty() {
            out.push(Inline::Text(TextRun {
                range: *line_range,
                content_lines: vec![*line_range],
            }));
            continue;
        }
        let mut cursor = 0usize;
        for (whole, target) in embeds {
            if whole.start > cursor {
                let piece =
                    ByteRange::new(line_range.start + cursor, line_range.start + whole.start);
                out.push(Inline::Text(TextRun {
                    range: piece,
                    content_lines: vec![piece],
                }));
            }
            let whole_range =
                ByteRange::new(line_range.start + whole.start, line_range.start + whole.end);
            let target_range = ByteRange::new(
                line_range.start + target.start,
                line_range.start + target.end,
            );
            let target_text = content
                .get(target_range.start..target_range.end)
                .unwrap_or("")
                .to_string();
            out.push(Inline::Image(ImageM {
                sm: RevealSm::new(RevealState::Rendered),
                range: whole_range,
                alt: ByteRange::new(whole_range.end, whole_range.end),
                target: target_range,
                target_text,
                is_wikilink: true,
                line: super::line_at(starts, whole_range.start),
                content_lines: vec![whole_range],
            }));
            cursor = whole.end;
        }
        if cursor < line_text.len() {
            let piece = ByteRange::new(line_range.start + cursor, line_range.end);
            out.push(Inline::Text(TextRun {
                range: piece,
                content_lines: vec![piece],
            }));
        }
    }
}

/// Finds every non-overlapping `![[...]]` occurrence in `line` (byte
/// offsets relative to `line`'s own start), returning `(whole, target)`
/// pairs. A `![[` with no matching `]]` before the line ends is left
/// unmatched — degrades to plain text, CommonMark's own "an unmatched
/// delimiter is literal" posture.
fn find_embeds_in_line(line: &str) -> Vec<(std::ops::Range<usize>, std::ops::Range<usize>)> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while let Some(rel) = line[i..].find("![[") {
        let start = i + rel;
        let target_start = start + 3;
        match line[target_start..].find("]]") {
            Some(rel_close) => {
                let target_end = target_start + rel_close;
                let end = target_end + 2;
                out.push((start..end, target_start..target_end));
                i = end;
            }
            None => break,
        }
    }
    out
}
