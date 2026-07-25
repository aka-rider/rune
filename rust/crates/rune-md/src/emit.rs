//! Emitter (plan Context, "Emit -> wrap -> snapshot"): walks the `Block`/
//! `Inline` tree in model-line order, producing one `SyntaxLine` per buffer
//! line plus a `SyntaxSnapshot` for buffer<->syntax coordinate conversion —
//! a structural port of `pkg/editor/display/{markdown_block,syntax_map,
//! syntax_snapshot,cellmap}.go`.
//!
//! Concealment is physical here, uniformly for block markers AND inline
//! delimiters: a `Rendered` element's marker/delimiter bytes are dropped
//! from the emitted text (recorded as a hidden range for coordinate
//! conversion) rather than kept-but-restyled. This is a deliberate
//! simplification of Go's model, where block-level markers (heading `"## "`,
//! blockquote `"> "`) stay in the emitted text and are hidden only by the
//! renderer, while inline delimiters are physically dropped — two policies
//! for one concept. Phase 1 unifies them: `Rendered` always means "the
//! markup bytes are not part of the syntax-space text", block or inline
//! alike, consistent with the plan's single `RevealState` used everywhere.
//!
//! Nested styling (bold-inside-italic) falls out of the tree via `StyleCtx`,
//! an accumulator that lives only for the duration of the walk — no
//! `InlineMarks` bitfield is stored on any `SyntaxSpan` (plan Context:
//! "Nested styling ... falls out of the tree via the Emitter's style stack —
//! no `InlineMarks` bitfield").

use crate::element::block::{Block, CodeFenceM, ListItemM, VerbatimKind};
use crate::element::inline::{EmphasisKind, Inline};
use crate::element::{ByteRange, RevealState};
use crate::parse::{line_at, line_end_at, line_starts};
use rune_core::coords::{BufferPoint, SyntaxPoint};

/// Semantic style tag — "what kind of markdown token is this", not a
/// rendered `ratatui::Style`. The lipgloss/ratatui-equivalent theme lives in
/// rune-tui (plan Context: "the lipgloss-equivalent theme lives in
/// rune-tui").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StyleId {
    Text,
    H1,
    H2,
    H3,
    H4,
    H5,
    H6,
    Bold,
    Italic,
    BoldItalic,
    Strike,
    BoldStrike,
    ItalicStrike,
    BoldItalicStrike,
    Code,
    CodeFence,
    Link,
    WikiLink,
    Blockquote,
    ListMarker,
    TaskMarker,
    Hr,
    FrontmatterDim,
    Verbatim,
}

/// Per-visual-char buffer offset, `-1` for decorative/padding cells with no
/// buffer correspondence — port of `pkg/editor/display/cellmap.go`'s
/// `CellMapping`. Phase 1 never produces `-1` (no decorative padding in this
/// crate yet); the type still carries it so the proptest invariant
/// ("entries are -1 or valid char boundaries") is meaningful, and so a
/// future decorative producer doesn't need a type change.
pub type CellMap = Vec<i64>;

#[derive(Clone, Debug)]
pub struct SyntaxSpan {
    pub text: String,
    pub style: StyleId,
    pub state: RevealState,
    pub buffer_start: usize,
    pub buffer_end: usize,
    /// Only `Some` for `Rendered` spans (plan: "`cell_map` only for
    /// Rendered spans, one buffer offset per char").
    pub cell_map: Option<CellMap>,
}

#[derive(Clone, Debug, Default)]
pub struct SyntaxLine {
    pub spans: Vec<SyntaxSpan>,
}

#[derive(Clone, Copy, Debug)]
struct OffsetDelta {
    buffer_offset: usize,
    delta: usize,
}

#[derive(Clone, Copy, Debug)]
struct HiddenRange {
    start: usize,
    end: usize,
    clamp_to: usize,
}

#[derive(Clone, Debug, Default)]
struct LineConversion {
    deltas: Vec<OffsetDelta>,
    hidden: Vec<HiddenRange>,
}

/// Coordinate conversion between Buffer Space and Syntax Space — port of
/// `pkg/editor/display/syntax_snapshot.go:35-97`. Positions inside hidden
/// delimiters clamp to the nearest cursor-legal syntax position.
#[derive(Clone, Debug, Default)]
pub struct SyntaxSnapshot {
    line_convs: Vec<LineConversion>,
}

impl SyntaxSnapshot {
    pub fn buffer_to_syntax(&self, bp: BufferPoint) -> SyntaxPoint {
        let Some(lc) = self.line_convs.get(bp.line) else {
            return SyntaxPoint {
                line: bp.line,
                col: bp.col,
            };
        };
        if lc.deltas.is_empty() {
            return SyntaxPoint {
                line: bp.line,
                col: bp.col,
            };
        }
        let col = clamp_col(bp.col, &lc.hidden);
        let mut delta = 0usize;
        for d in &lc.deltas {
            if d.buffer_offset <= col {
                delta = d.delta;
            } else {
                break;
            }
        }
        SyntaxPoint {
            line: bp.line,
            col: col.saturating_sub(delta),
        }
    }

    pub fn syntax_to_buffer(&self, sp: SyntaxPoint) -> BufferPoint {
        let Some(lc) = self.line_convs.get(sp.line) else {
            return BufferPoint {
                line: sp.line,
                col: sp.col,
            };
        };
        if lc.deltas.is_empty() {
            return BufferPoint {
                line: sp.line,
                col: sp.col,
            };
        }
        let mut delta = 0usize;
        for d in &lc.deltas {
            let syntax_at_entry = d.buffer_offset.saturating_sub(d.delta);
            if syntax_at_entry <= sp.col {
                delta = d.delta;
            } else {
                break;
            }
        }
        BufferPoint {
            line: sp.line,
            col: sp.col + delta,
        }
    }
}

fn clamp_col(col: usize, hidden: &[HiddenRange]) -> usize {
    for h in hidden {
        if col >= h.start && col < h.end {
            return h.clamp_to;
        }
        if h.start > col {
            break;
        }
    }
    col
}

/// Per-parent accumulator resolving nested emphasis to one `StyleId` at leaf
/// emission time — the "style stack", kept only for the duration of the
/// walk. A non-emphasis ancestor (`Link`/`WikiLink`/`Code`) overrides and
/// ignores any accumulated emphasis (Phase-1 simplification: a link's own
/// color wins over surrounding bold/italic).
#[derive(Clone, Copy, Debug)]
enum StyleCtx {
    Emphasis {
        bold: bool,
        italic: bool,
        strike: bool,
    },
    Override(StyleId),
}

impl Default for StyleCtx {
    fn default() -> Self {
        StyleCtx::Emphasis {
            bold: false,
            italic: false,
            strike: false,
        }
    }
}

impl StyleCtx {
    fn with_kind(self, kind: EmphasisKind) -> StyleCtx {
        match self {
            StyleCtx::Override(_) => self,
            StyleCtx::Emphasis {
                bold,
                italic,
                strike,
            } => {
                let (bold, italic, strike) = match kind {
                    EmphasisKind::Bold => (true, italic, strike),
                    EmphasisKind::Italic => (bold, true, strike),
                    EmphasisKind::Strike => (bold, italic, true),
                    EmphasisKind::BoldItalic => (true, true, strike),
                };
                StyleCtx::Emphasis {
                    bold,
                    italic,
                    strike,
                }
            }
        }
    }

    fn resolve(self) -> StyleId {
        match self {
            StyleCtx::Override(s) => s,
            StyleCtx::Emphasis {
                bold,
                italic,
                strike,
            } => match (bold, italic, strike) {
                (false, false, false) => StyleId::Text,
                (true, false, false) => StyleId::Bold,
                (false, true, false) => StyleId::Italic,
                (false, false, true) => StyleId::Strike,
                (true, true, false) => StyleId::BoldItalic,
                (true, false, true) => StyleId::BoldStrike,
                (false, true, true) => StyleId::ItalicStrike,
                (true, true, true) => StyleId::BoldItalicStrike,
            },
        }
    }
}

/// Port of `pkg/editor/display/cellmap.go:buildInlineCellMap`: one entry per
/// visual char, the absolute buffer offset it maps back to.
fn build_cell_map(content_start: usize, text: &str) -> CellMap {
    let mut cm = Vec::with_capacity(text.chars().count());
    let mut i = 0usize;
    for ch in text.chars() {
        cm.push((content_start + i) as i64);
        i += ch.len_utf8();
    }
    cm
}

fn heading_style(level: u8) -> StyleId {
    match level {
        1 => StyleId::H1,
        2 => StyleId::H2,
        3 => StyleId::H3,
        4 => StyleId::H4,
        5 => StyleId::H5,
        _ => StyleId::H6,
    }
}

/// The workhorse: split an absolute buffer range across the source lines it
/// covers and push one `SyntaxSpan` per line-slice. Builds a `cell_map` only
/// for `Rendered` spans (their text is always a direct, contiguous slice of
/// the buffer at this call site — concealed content minus its delimiters).
fn push_span_split_by_line(
    content: &str,
    starts: &[usize],
    range: ByteRange,
    style: StyleId,
    state: RevealState,
    out: &mut [Vec<SyntaxSpan>],
    touched: &mut [bool],
) {
    if range.is_empty() {
        return;
    }
    let first_line = line_at(starts, range.start);
    let last_line = line_at(starts, range.end.saturating_sub(1).max(range.start));
    for line in first_line..=last_line {
        let line_start = starts.get(line).copied().unwrap_or(0);
        let line_end = line_end_at(content.len(), starts, line);
        let seg_start = range.start.max(line_start);
        let seg_end = range.end.min(line_end);
        if seg_end <= seg_start {
            continue;
        }
        let Some(text) = content.get(seg_start..seg_end) else {
            continue;
        };
        let cell_map = (state == RevealState::Rendered).then(|| build_cell_map(seg_start, text));
        if let Some(bucket) = out.get_mut(line) {
            bucket.push(SyntaxSpan {
                text: text.to_string(),
                style,
                state,
                buffer_start: seg_start,
                buffer_end: seg_end,
                cell_map,
            });
        }
        if let Some(t) = touched.get_mut(line) {
            *t = true;
        }
    }
}

/// Records an absolute buffer range as hidden (delimiter bytes dropped from
/// the emitted text). Phase-1 token scope never produces a multi-line
/// delimiter (`"## "`, `"> "`, `"**"`, `"["`, backticks, fence lines are all
/// single-line), so this only files under the line `range.start` is on.
fn push_hidden(hidden: &mut [Vec<(usize, usize)>], starts: &[usize], range: ByteRange) {
    if range.is_empty() {
        return;
    }
    let line = line_at(starts, range.start);
    if let Some(bucket) = hidden.get_mut(line) {
        bucket.push((range.start, range.end));
    }
}

fn mark_touched_range(touched: &mut [bool], starts: &[usize], range: ByteRange) {
    let first_line = line_at(starts, range.start);
    let last_line = line_at(starts, range.end.saturating_sub(1).max(range.start));
    for line in first_line..=last_line {
        if let Some(t) = touched.get_mut(line) {
            *t = true;
        }
    }
}

fn list_marker_style(item: &ListItemM) -> StyleId {
    if item.task.is_some() {
        StyleId::TaskMarker
    } else {
        StyleId::ListMarker
    }
}

fn emit_code_fence(
    content: &str,
    starts: &[usize],
    cf: &CodeFenceM,
    out: &mut [Vec<SyntaxSpan>],
    hidden: &mut [Vec<(usize, usize)>],
    touched: &mut [bool],
) {
    if cf.sm.state() == RevealState::Revealed {
        push_span_split_by_line(
            content,
            starts,
            cf.range,
            StyleId::CodeFence,
            RevealState::Revealed,
            out,
            touched,
        );
        return;
    }
    if let Some(open) = cf.fence_open {
        push_hidden(hidden, starts, open);
        mark_touched_range(touched, starts, open);
    }
    if let Some(close) = cf.fence_close {
        push_hidden(hidden, starts, close);
        mark_touched_range(touched, starts, close);
    }
    push_span_split_by_line(
        content,
        starts,
        cf.content,
        StyleId::CodeFence,
        RevealState::Rendered,
        out,
        touched,
    );
}

fn emit_list_item(
    content: &str,
    starts: &[usize],
    item: &ListItemM,
    out: &mut [Vec<SyntaxSpan>],
    hidden: &mut [Vec<(usize, usize)>],
    touched: &mut [bool],
) {
    if item.sm.state() == RevealState::Revealed {
        push_span_split_by_line(
            content,
            starts,
            item.marker,
            list_marker_style(item),
            RevealState::Revealed,
            out,
            touched,
        );
    } else {
        push_hidden(hidden, starts, item.marker);
        mark_touched_range(touched, starts, item.marker);
    }
    for c in &item.children {
        emit_block(content, starts, c, out, hidden, touched);
    }
}

fn verbatim_style(kind: VerbatimKind) -> StyleId {
    match kind {
        VerbatimKind::Table | VerbatimKind::Html | VerbatimKind::Math | VerbatimKind::Unknown => {
            StyleId::Verbatim
        }
    }
}

fn emit_block(
    content: &str,
    starts: &[usize],
    block: &Block,
    out: &mut [Vec<SyntaxSpan>],
    hidden: &mut [Vec<(usize, usize)>],
    touched: &mut [bool],
) {
    match block {
        Block::Paragraph(p) => {
            emit_inlines(
                content,
                starts,
                &p.inlines,
                StyleCtx::default(),
                out,
                hidden,
                touched,
            );
        }
        Block::Heading(h) => {
            if h.sm.state() == RevealState::Revealed {
                push_span_split_by_line(
                    content,
                    starts,
                    h.range,
                    heading_style(h.level),
                    RevealState::Revealed,
                    out,
                    touched,
                );
            } else {
                push_hidden(hidden, starts, h.marker);
                mark_touched_range(touched, starts, h.marker);
                emit_inlines(
                    content,
                    starts,
                    &h.inlines,
                    StyleCtx::default(),
                    out,
                    hidden,
                    touched,
                );
            }
        }
        Block::Blockquote(bq) => {
            for m in &bq.markers {
                if m.sm.state() == RevealState::Revealed {
                    push_span_split_by_line(
                        content,
                        starts,
                        m.marker,
                        StyleId::Blockquote,
                        RevealState::Revealed,
                        out,
                        touched,
                    );
                } else {
                    push_hidden(hidden, starts, m.marker);
                    mark_touched_range(touched, starts, m.marker);
                }
            }
            for c in &bq.children {
                emit_block(content, starts, c, out, hidden, touched);
            }
        }
        Block::CodeFence(cf) => emit_code_fence(content, starts, cf, out, hidden, touched),
        Block::List(list) => {
            for item in &list.items {
                emit_list_item(content, starts, item, out, hidden, touched);
            }
        }
        Block::ThematicBreak(hr) => {
            if hr.sm.state() == RevealState::Revealed {
                push_span_split_by_line(
                    content,
                    starts,
                    hr.range,
                    StyleId::Hr,
                    RevealState::Revealed,
                    out,
                    touched,
                );
            } else {
                push_hidden(hidden, starts, hr.range);
                mark_touched_range(touched, starts, hr.range);
            }
        }
        Block::Frontmatter(fm) => {
            push_span_split_by_line(
                content,
                starts,
                fm.range,
                StyleId::FrontmatterDim,
                RevealState::Revealed,
                out,
                touched,
            );
        }
        Block::Verbatim(v) => {
            push_span_split_by_line(
                content,
                starts,
                v.range,
                verbatim_style(v.kind),
                RevealState::Revealed,
                out,
                touched,
            );
        }
    }
}

fn link_delims(range: ByteRange, children: &[Inline]) -> (ByteRange, ByteRange) {
    let open_end = children
        .first()
        .map(|c| c.range().start)
        .unwrap_or(range.end)
        .max(range.start)
        .min(range.end);
    let close_start = children
        .last()
        .map(|c| c.range().end)
        .unwrap_or(range.start)
        .max(range.start)
        .min(range.end);
    (
        ByteRange::new(range.start, open_end),
        ByteRange::new(close_start, range.end),
    )
}

fn emit_inlines(
    content: &str,
    starts: &[usize],
    inlines: &[Inline],
    style_ctx: StyleCtx,
    out: &mut [Vec<SyntaxSpan>],
    hidden: &mut [Vec<(usize, usize)>],
    touched: &mut [bool],
) {
    for inl in inlines {
        emit_inline(content, starts, inl, style_ctx, out, hidden, touched);
    }
}

fn emit_inline(
    content: &str,
    starts: &[usize],
    inl: &Inline,
    style_ctx: StyleCtx,
    out: &mut [Vec<SyntaxSpan>],
    hidden: &mut [Vec<(usize, usize)>],
    touched: &mut [bool],
) {
    match inl {
        Inline::Text(t) => {
            push_span_split_by_line(
                content,
                starts,
                t.range,
                style_ctx.resolve(),
                RevealState::Revealed,
                out,
                touched,
            );
        }
        Inline::Emphasis(m) => {
            let child_ctx = style_ctx.with_kind(m.kind);
            if m.sm.state() == RevealState::Revealed {
                push_span_split_by_line(
                    content,
                    starts,
                    m.range,
                    child_ctx.resolve(),
                    RevealState::Revealed,
                    out,
                    touched,
                );
            } else {
                push_hidden(hidden, starts, m.open);
                mark_touched_range(touched, starts, m.open);
                emit_inlines(
                    content,
                    starts,
                    &m.children,
                    child_ctx,
                    out,
                    hidden,
                    touched,
                );
                push_hidden(hidden, starts, m.close);
                mark_touched_range(touched, starts, m.close);
            }
        }
        Inline::Code(m) => {
            if m.sm.state() == RevealState::Revealed {
                push_span_split_by_line(
                    content,
                    starts,
                    m.range,
                    StyleId::Code,
                    RevealState::Revealed,
                    out,
                    touched,
                );
            } else {
                push_hidden(hidden, starts, m.open);
                mark_touched_range(touched, starts, m.open);
                push_span_split_by_line(
                    content,
                    starts,
                    m.content,
                    StyleId::Code,
                    RevealState::Rendered,
                    out,
                    touched,
                );
                push_hidden(hidden, starts, m.close);
                mark_touched_range(touched, starts, m.close);
            }
        }
        Inline::Link(m) => {
            if m.sm.state() == RevealState::Revealed {
                push_span_split_by_line(
                    content,
                    starts,
                    m.range,
                    StyleId::Link,
                    RevealState::Revealed,
                    out,
                    touched,
                );
            } else {
                let (open, close) = link_delims(m.range, &m.text);
                push_hidden(hidden, starts, open);
                mark_touched_range(touched, starts, open);
                emit_inlines(
                    content,
                    starts,
                    &m.text,
                    StyleCtx::Override(StyleId::Link),
                    out,
                    hidden,
                    touched,
                );
                push_hidden(hidden, starts, close);
                mark_touched_range(touched, starts, close);
            }
        }
        Inline::WikiLink(m) => {
            if m.sm.state() == RevealState::Revealed {
                push_span_split_by_line(
                    content,
                    starts,
                    m.range,
                    StyleId::WikiLink,
                    RevealState::Revealed,
                    out,
                    touched,
                );
            } else {
                let open = ByteRange::new(
                    m.range.start,
                    m.label.start.max(m.range.start).min(m.range.end),
                );
                let close =
                    ByteRange::new(m.label.end.max(m.range.start).min(m.range.end), m.range.end);
                push_hidden(hidden, starts, open);
                mark_touched_range(touched, starts, open);
                push_span_split_by_line(
                    content,
                    starts,
                    m.label,
                    StyleId::WikiLink,
                    RevealState::Rendered,
                    out,
                    touched,
                );
                push_hidden(hidden, starts, close);
                mark_touched_range(touched, starts, close);
            }
        }
    }
}

fn fill_line_gaps(content: &str, starts: &[usize], touched: &[bool], out: &mut [Vec<SyntaxSpan>]) {
    for (line, is_touched) in touched.iter().enumerate() {
        if *is_touched {
            continue;
        }
        let line_start = starts.get(line).copied().unwrap_or(0);
        let line_end = line_end_at(content.len(), starts, line).max(line_start);
        let Some(text) = content.get(line_start..line_end) else {
            continue;
        };
        if let Some(bucket) = out.get_mut(line) {
            bucket.push(SyntaxSpan {
                text: text.to_string(),
                style: StyleId::Text,
                state: RevealState::Revealed,
                buffer_start: line_start,
                buffer_end: line_end,
                cell_map: None,
            });
        }
    }
}

fn build_line_conversions(starts: &[usize], hidden: &[Vec<(usize, usize)>]) -> Vec<LineConversion> {
    let mut convs = Vec::with_capacity(hidden.len());
    for (line, ranges) in hidden.iter().enumerate() {
        let line_start = starts.get(line).copied().unwrap_or(0);
        let mut rel: Vec<(usize, usize)> = ranges
            .iter()
            .map(|&(s, e)| (s.saturating_sub(line_start), e.saturating_sub(line_start)))
            .collect();
        rel.sort_by_key(|&(s, _)| s);

        let mut deltas = Vec::with_capacity(rel.len());
        let mut hidden_ranges = Vec::with_capacity(rel.len());
        let mut accum = 0usize;
        for (s, e) in rel {
            if e <= s {
                continue;
            }
            accum += e - s;
            hidden_ranges.push(HiddenRange {
                start: s,
                end: e,
                clamp_to: e,
            });
            deltas.push(OffsetDelta {
                buffer_offset: e,
                delta: accum,
            });
        }
        convs.push(LineConversion {
            deltas,
            hidden: hidden_ranges,
        });
    }
    convs
}

/// The crate's one Emit entry point: `Block` tree -> per-line `SyntaxLine`s
/// and a `SyntaxSnapshot` for coordinate conversion. `DocMachine::snapshot`
/// (WP4) is the only caller.
pub fn emit(content: &str, blocks: &[Block]) -> (Vec<SyntaxLine>, SyntaxSnapshot) {
    let starts = line_starts(content);
    let mut spans: Vec<Vec<SyntaxSpan>> = vec![Vec::new(); starts.len()];
    let mut hidden: Vec<Vec<(usize, usize)>> = vec![Vec::new(); starts.len()];
    let mut touched = vec![false; starts.len()];

    for b in blocks {
        emit_block(content, &starts, b, &mut spans, &mut hidden, &mut touched);
    }
    fill_line_gaps(content, &starts, &touched, &mut spans);

    let lines: Vec<SyntaxLine> = spans
        .into_iter()
        .map(|spans| SyntaxLine { spans })
        .collect();
    let line_convs = build_line_conversions(&starts, &hidden);
    (lines, SyntaxSnapshot { line_convs })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::element::doc::DocMachine;
    use rune_core::buffer::Buffer;
    use rune_core::cursor::CursorSet;

    fn synced(content: &str, cursor_offset: usize, focused: bool) -> (Buffer, DocMachine) {
        let buf = Buffer::new(content);
        let mut doc = DocMachine::new();
        doc.set_focus(focused);
        doc.sync_content(&buf);
        let cursors = CursorSet::new(cursor_offset);
        doc.sync_cursors(&buf, &cursors);
        (buf, doc)
    }

    #[test]
    fn heading_marker_hidden_when_not_on_cursor_line() {
        let (buf, doc) = synced("# hi\nsecond\n", 8, true);
        let (lines, _snap) = emit(buf.content(), doc.blocks());
        let joined: String = lines[0].spans.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(joined, "hi", "marker must be concealed off-cursor-line");
    }

    #[test]
    fn heading_marker_revealed_on_cursor_line() {
        let (buf, doc) = synced("# hi\nsecond\n", 0, true);
        let (lines, _snap) = emit(buf.content(), doc.blocks());
        let joined: String = lines[0].spans.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(joined, "# hi");
    }

    #[test]
    fn unfocused_conceals_everything() {
        let (buf, doc) = synced("# hi\n", 0, false);
        let (lines, _snap) = emit(buf.content(), doc.blocks());
        let joined: String = lines[0].spans.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(joined, "hi");
    }

    #[test]
    fn code_fence_whole_block_reveals_as_unit() {
        let content = "```rust\nfn f() {}\n```\n";
        let (buf, doc) = synced(content, content.find("fn").unwrap(), true);
        let (lines, _snap) = emit(buf.content(), doc.blocks());
        // Cursor is on line 1 (the content line); the whole 3-line fence
        // block must reveal, including the fence marker lines.
        assert_eq!(lines[0].spans[0].text, "```rust");
        assert_eq!(lines[1].spans[0].text, "fn f() {}");
        assert_eq!(lines[2].spans[0].text, "```");
    }

    #[test]
    fn code_fence_conceals_marker_lines_off_cursor() {
        let content = "```rust\nfn f() {}\n```\nafter\n";
        let (buf, doc) = synced(content, content.find("after").unwrap(), true);
        let (lines, _snap) = emit(buf.content(), doc.blocks());
        // Fence marker lines collapse to empty; content line shows verbatim.
        assert_eq!(
            lines[0]
                .spans
                .iter()
                .map(|s| s.text.as_str())
                .collect::<String>(),
            ""
        );
        assert_eq!(lines[1].spans[0].text, "fn f() {}");
    }

    #[test]
    fn bold_reveals_with_nested_link_as_a_unit() {
        let content = "**[bo*ld*](url)** end\n";
        let cursor = content.find("ld").unwrap();
        let (buf, doc) = synced(content, cursor, true);
        let (lines, _snap) = emit(buf.content(), doc.blocks());
        let joined: String = lines[0].spans.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(joined, "**[bo*ld*](url)** end");
    }

    #[test]
    fn bold_conceals_but_still_shows_nested_link_text() {
        let content = "**[bo*ld*](url)** end\n";
        let (buf, doc) = synced(content, content.len(), true); // cursor at " end", not inside bold
        let (lines, _snap) = emit(buf.content(), doc.blocks());
        let joined: String = lines[0].spans.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(joined, "bold end");
    }

    #[test]
    fn rendered_span_cell_map_offsets_are_within_range() {
        let content = "**bold** text\n";
        let (buf, doc) = synced(content, content.len(), true);
        let (lines, _snap) = emit(buf.content(), doc.blocks());
        for span in &lines[0].spans {
            if let Some(cm) = &span.cell_map {
                for &off in cm {
                    assert!(off == -1 || (off as usize) < span.buffer_end);
                    if off != -1 {
                        assert!((off as usize) >= span.buffer_start);
                    }
                }
            }
        }
    }

    #[test]
    fn buffer_to_syntax_roundtrip_on_cursor_legal_position() {
        // Cursor is at end-of-buffer, well outside "**bold**"'s range, so
        // the emphasis is concealed on line 0 and its "**" delimiters (buffer
        // cols [0,2) and [6,8)) are NOT cursor-legal. Buffer col 8 is the
        // space right after the closing "**", a position with no hidden
        // delimiter on either side — genuinely cursor-legal, so the
        // roundtrip must be exact (unlike a position inside a hidden range,
        // which only guarantees the weaker stability invariant the other
        // test below checks).
        let content = "**bold** text\n";
        let (buf, doc) = synced(content, content.len(), true);
        let (_lines, snap) = emit(buf.content(), doc.blocks());
        let bp = BufferPoint { line: 0, col: 8 }; // buffer col 8 = the space after "**bold**"
        let sp = snap.buffer_to_syntax(bp);
        let bp2 = snap.syntax_to_buffer(sp);
        assert_eq!(bp, bp2);
    }

    #[test]
    fn buffer_to_syntax_clamps_stably_inside_hidden_delimiter() {
        // Mirrors Go's FuzzSyntaxMapRoundtrip stability property: a
        // position inside a hidden delimiter range does NOT roundtrip to
        // itself, but the CLAMPED position it lands on must be idempotent.
        let content = "**bold** text\n";
        let (buf, doc) = synced(content, content.len(), true);
        let (_lines, snap) = emit(buf.content(), doc.blocks());
        let bp = BufferPoint { line: 0, col: 0 }; // inside the "**" open delimiter
        let sp = snap.buffer_to_syntax(bp);
        let bp2 = snap.syntax_to_buffer(sp);
        assert_ne!(
            bp, bp2,
            "col 0 sits inside a hidden delimiter, not cursor-legal"
        );
        let sp2 = snap.buffer_to_syntax(bp2);
        assert_eq!(
            sp, sp2,
            "the clamped position must be stable under a second round-trip"
        );
    }
}
