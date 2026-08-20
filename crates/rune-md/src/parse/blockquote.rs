use super::{ScanHint, line_end_at};
use crate::element::block::BlockquoteMarkerM;
use rune_syntax::element::{ByteRange, RevealSm, RevealState};

// For a nested blockquote (`"> > nested"`), comrak reports each depth's own
// `BlockQuote` node sourcepos starting right after the outer level's "> "
// prefix, but only on the node's overall start — not per continuation
// line. `hint` supplies the depth-aware scan start uniformly for every
// line instead.
pub(super) fn blockquote_markers(
    content: &str,
    starts: &[usize],
    range: ByteRange,
    hint: &ScanHint,
) -> Vec<BlockquoteMarkerM> {
    let first_line = super::line_at(starts, range.start);
    let last_line = super::line_at(starts, range.end.saturating_sub(1).max(range.start));
    let mut markers = Vec::new();
    for line in first_line..=last_line {
        let line_end = line_end_at(content.len(), starts, line);
        let scan_start = hint.start_for_line(starts, line);
        let Some(line_text) = content.get(scan_start..line_end.max(scan_start)) else {
            continue;
        };
        // A tab at column 0, 1, 2 or 3 always advances to column 4. A leading
        // run of whitespace therefore reaches an indentation of 3 or less only
        // if it is all spaces. Counting spaces and capping at 3 is CommonMark's
        // own "indented by at most 3" test for a repeated block-container marker.
        let ws_len = line_text.bytes().take_while(|&b| b == b' ').count().min(3);
        let Some(trimmed) = line_text.get(ws_len..) else {
            continue;
        };
        if let Some(rest) = trimmed.strip_prefix('>') {
            let mut marker_len = ws_len + 1;
            if rest.starts_with(' ') {
                marker_len += 1;
            }
            let marker_end = scan_start.saturating_add(marker_len).min(content.len());
            markers.push(BlockquoteMarkerM {
                sm: RevealSm::new(RevealState::Rendered),
                line: super::line_at(starts, scan_start),
                marker: ByteRange::new(scan_start, marker_end),
            });
        }
    }
    markers
}
