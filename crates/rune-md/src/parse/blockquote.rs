//! Blockquote marker derivation — split out from `block.rs` to keep it
//! under CONSTITUTION §1.6's 500-LoC limit.

use super::{ScanHint, line_end_at};
use crate::element::block::BlockquoteMarkerM;
use rune_syntax::element::{ByteRange, RevealSm, RevealState};

/// Derives one `"> "` marker range per source line covered by a blockquote's
/// range — there is no dedicated comrak node for the marker itself, so this
/// scans the raw text (plan Context "Parse": unmodeled delimiters are
/// derived, never invented from thin air).
///
/// MAJOR 4 fix: for a NESTED blockquote (`"> > nested"`), comrak reports
/// each depth's own `BlockQuote` node sourcepos starting right AFTER the
/// outer level's `"> "` prefix on line 0 — verified empirically: for
/// `"> > nested quote\n"` the outer node's sourcepos is `1:1-1:16` and the
/// inner's is `1:3-1:16` (column 3 = byte offset 2, right past the outer
/// `"> "`). But that per-line signal only exists for line 0 — a multi-line
/// nested blockquote (`"> > nested\n> > nested"`) needs the SAME
/// depth-aware scan-start on every continuation line too, which comrak's
/// sourcepos doesn't give us (only the node's overall start/end). `hint`
/// (built by the caller from the immediately enclosing depth's own
/// just-computed markers) supplies it uniformly for every line, line 0
/// included — see `ScanHint`'s docs.
pub(super) fn blockquote_markers(
    content: &str,
    starts: &[usize],
    range: ByteRange,
    hint: &ScanHint,
) -> Vec<BlockquoteMarkerM> {
    // Iterated and clamped by `starts` — a `"> "` marker repeats once per
    // line, matching `ScanHint`'s own `starts`-keyed lookups (see
    // `block.rs`'s `BlockQuote` arm).
    let first_line = super::line_at(starts, range.start);
    let last_line = super::line_at(starts, range.end.saturating_sub(1).max(range.start));
    let mut markers = Vec::new();
    for line in first_line..=last_line {
        let line_end = line_end_at(content.len(), starts, line);
        let scan_start = hint.start_for_line(starts, line);
        let Some(line_text) = content.get(scan_start..line_end.max(scan_start)) else {
            continue;
        };
        // RESIDUAL PRODUCER fix (verification round 3, ">]\n\t>"): only
        // SPACES (never a tab), capped at 3, count as marker-prefix
        // indentation — CommonMark's own indentation rule for a repeated
        // block-container marker. `str::trim_start()` strips ANY
        // whitespace including tabs, so it used to also recognize a
        // tab-indented line ("\t>") as a repeated ">" marker; comrak
        // itself does NOT (a leading tab represents 4 columns, past the
        // 3-space budget, so it treats the line as lazy-continuation
        // paragraph TEXT instead). That mismatch meant this scan and the
        // paragraph's own Text node both claimed the same "> " bytes — a
        // producer-bug double-claim `push_span_split_by_line` now catches
        // via `assert_invariant`. Capping at 3 spaces (not just excluding
        // tabs outright) keeps a legitimately 1-3-space-indented
        // continuation ("   > cont") recognized, matching comrak exactly.
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
                // Stored for the cursor-reveal decide policy, which
                // compares against the cursor's own buffer row.
                line: super::line_at(starts, scan_start),
                marker: ByteRange::new(scan_start, marker_end),
            });
        }
    }
    markers
}
