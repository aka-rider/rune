use super::node_range;
use crate::element::inline::{ImageM, Inline, TextRun};
use comrak::nodes::AstNode;
use rune_syntax::element::{ByteRange, RevealSm, RevealState};

const EMBED_OPEN: &str = "![[";
const EMBED_CLOSE: &str = "]]";

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

// comrak's own wikilink trigger has a `within_brackets` guard that
// suppresses the node entirely under a leading `!`, so `![[note]]` parses
// as plain text with no dedicated AST node at all.
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

fn split_text_run_embeds(content: &str, starts: &[usize], t: TextRun, out: &mut Vec<Inline>) {
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

pub(super) struct LineEmbed<'a> {
    whole: std::ops::Range<usize>,
    target: std::ops::Range<usize>,
    target_text: &'a str,
}

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
