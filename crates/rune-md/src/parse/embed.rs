//! Image range derivation and `![[target]]` embed recovery — split out of
//! `parse::inline` to keep that module under the 500-line budget.
//! `build_inline`'s `InlineKind::Image` arm calls
//! [`image_alt_range`]/[`image_target_range`] directly; `build_inlines`
//! funnels its own output through [`recover_embeds`] before returning, on
//! both its normal fall-through and its multiline-wikilink-recovery early
//! return.

use super::node_range;
use crate::element::inline::{ImageM, Inline, TextRun};
use comrak::nodes::AstNode;
use rune_syntax::element::{ByteRange, RevealSm, RevealState};

/// The opening delimiter of a wiki-style image embed. Named so the scan and
/// its fast-path guard can never drift apart on the literal.
const EMBED_OPEN: &str = "![[";
/// The closing delimiter of a wiki-style image embed.
const EMBED_CLOSE: &str = "]]";

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
    // Fast path: almost every text run in almost every document contains no
    // embed at all. Splitting one into per-line runs emits byte-identical
    // spans (emission walks `content_lines`, never `range`), so the split
    // would buy nothing while allocating a fresh run per line on the parse
    // path every keystroke re-runs. Hand an embed-free run straight back.
    let has_embed = t.content_lines.iter().any(|r| {
        content
            .get(r.start..r.end)
            .is_some_and(|line| line.contains(EMBED_OPEN))
    });
    if !has_embed {
        out.push(Inline::Text(t));
        return;
    }

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
        for embed in embeds {
            if embed.whole.start > cursor {
                let piece = ByteRange::new(
                    line_range.start + cursor,
                    line_range.start + embed.whole.start,
                );
                out.push(Inline::Text(TextRun {
                    range: piece,
                    content_lines: vec![piece],
                }));
            }
            let whole_range = ByteRange::new(
                line_range.start + embed.whole.start,
                line_range.start + embed.whole.end,
            );
            let target_range = ByteRange::new(
                line_range.start + embed.target.start,
                line_range.start + embed.target.end,
            );
            out.push(Inline::Image(ImageM {
                sm: RevealSm::new(RevealState::Rendered),
                range: whole_range,
                // `![[x]]` carries no alt text at all. The empty range sits
                // where alt content would begin — matching the childless
                // `![](url)` convention of anchoring an absent alt at its
                // own opening delimiter, rather than at the token's end.
                alt: ByteRange::new(target_range.start, target_range.start),
                target: target_range,
                target_text: embed.target_text.to_string(),
                is_wikilink: true,
                line: super::line_at(starts, whole_range.start),
                content_lines: vec![whole_range],
            }));
            cursor = embed.whole.end;
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

/// One `![[target]]` occurrence within a single line, in byte offsets
/// relative to that line's own start. `target_text` is carried alongside
/// the ranges because this is the one place the slice is provably in
/// bounds — deriving it again at the emit site would need a fallback for a
/// case that cannot arise.
pub(super) struct LineEmbed<'a> {
    whole: std::ops::Range<usize>,
    target: std::ops::Range<usize>,
    target_text: &'a str,
}

/// Finds every non-overlapping `![[...]]` occurrence in `line`. A `![[`
/// with no matching `]]` before the line ends is left unmatched — it
/// degrades to plain text, CommonMark's own "an unmatched delimiter is
/// literal" posture.
///
/// Every slice goes through `get`, never `[]`: an out-of-bounds or
/// non-char-boundary offset ends the scan instead of panicking. The
/// delimiters are ASCII so the offsets are in fact always char boundaries,
/// but a halt beats a panic even on the impossible branch, and nothing
/// here has to rely on that reasoning holding.
fn find_embeds_in_line(line: &str) -> Vec<LineEmbed<'_>> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while let Some(rel) = line.get(i..).and_then(|rest| rest.find(EMBED_OPEN)) {
        let start = i + rel;
        let target_start = start + EMBED_OPEN.len();
        let Some(after_open) = line.get(target_start..) else {
            break;
        };
        let Some(rel_close) = after_open.find(EMBED_CLOSE) else {
            break;
        };
        let target_end = target_start + rel_close;
        let end = target_end + EMBED_CLOSE.len();
        let Some(target_text) = line.get(target_start..target_end) else {
            break;
        };
        out.push(LineEmbed {
            whole: start..end,
            target: target_start..target_end,
            target_text,
        });
        i = end;
    }
    out
}
