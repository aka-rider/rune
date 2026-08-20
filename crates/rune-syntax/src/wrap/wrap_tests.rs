use super::*;
use crate::scope::ScopeId;

const TEXT: ScopeId = ScopeId(0);
const CODE: ScopeId = ScopeId(1);

/// Splits `content` on `\n` into one `Identical`, whole-line `SyntaxLine`
/// per line (dropping the line terminator from the visible range, an
/// empty line becoming a `SyntaxLine::default()`) — a minimal stand-in
/// for a producer's emitted output. Builds this crate's own test inputs
/// directly rather than routing through `rune-md`'s `DocMachine`/`emit`
/// (WP3: `rune-syntax` must stand up without depending on `rune-md`);
/// these tests exercise `WrapMap`'s own contract only, not concealment.
fn plain_lines(content: &str) -> Vec<SyntaxLine> {
    let starts = rune_core::buffer::line_starts(content);
    starts
        .iter()
        .enumerate()
        .map(|(i, &s)| {
            let e = starts.get(i + 1).copied().unwrap_or(content.len());
            let line_end = if e > s && content.as_bytes().get(e - 1) == Some(&b'\n') {
                e - 1
            } else {
                e
            };
            if line_end > s {
                SyntaxLine {
                    spans: vec![SyntaxSpan::Identical {
                        scope: TEXT,
                        range: s..line_end,
                    }],
                    table: None,
                    decor: None,
                }
            } else {
                SyntaxLine::default()
            }
        })
        .collect()
}

fn wrap_lines(content: &str, width: u16) -> WrapSnapshot {
    let lines = plain_lines(content);
    WrapMap::new(width).sync(content, &lines)
}

#[test]
fn short_line_is_a_single_segment() {
    let wrap = wrap_lines("hello world\n", 80);
    assert_eq!(wrap.total_rows(), 2); // "hello world" + the trailing empty line
    assert_eq!(wrap.segment_len_at(0), 11);
}

#[test]
fn long_line_breaks_before_width_limit_at_the_last_space_seen() {
    // The greedy loop always backs off to the
    // last space it has seen so far whenever more text remains past the
    // width-fitting cutoff — even when the width-fitting cutoff itself
    // lands cleanly at a word boundary. width=11 fits "hello world"
    // (11 cols) exactly, but "again" still remains, so the segment
    // backs off to right after the FIRST space: "hello ".
    let content = "hello world again\n";
    let wrap = wrap_lines(content, 11);
    let seg0 = &wrap.segments()[0];
    let text: String = seg0.spans.iter().map(|s| s.text(content)).collect();
    assert_eq!(text, "hello ");

    // No segment on this line exceeds the configured width, and the
    // segments concatenate back to the exact original line text.
    let line0_segments: Vec<&WrapSegment> = wrap
        .segments()
        .iter()
        .filter(|s| s.model_line == 0)
        .collect();
    let mut joined = String::new();
    for seg in &line0_segments {
        let seg_text: String = seg.spans.iter().map(|s| s.text(content)).collect();
        joined.push_str(&seg_text);
    }
    assert_eq!(joined, "hello world again");
}

#[test]
fn rendered_span_text_and_cell_map_split_together_buffer_range_stays_whole() {
    // A Substituted span's TEXT and CellMap DO
    // get sliced at a wrap break, same as any other span — only its
    // `range` is left at the full original range, because a
    // Substituted span's text isn't byte-for-byte its buffer range once
    // delimiters are dropped. Hand-built (see `plain_lines`'s docs): a
    // concealed inline-code span, its delimiting backticks NOT part of
    // any span's range (they'd be a separate hidden range in a real
    // producer's `SyntaxSnapshot`, irrelevant to `WrapMap`).
    let content = "x `aaaaaaaaaaaaaaaaaaaa` y\n";
    let code_text = "aaaaaaaaaaaaaaaaaaaa";
    let line0 = SyntaxLine {
        spans: vec![
            SyntaxSpan::Identical {
                scope: TEXT,
                range: 0..2,
            },
            SyntaxSpan::substituted(3, code_text.to_string(), CODE, 3..23),
            SyntaxSpan::Identical {
                scope: TEXT,
                range: 24..26,
            },
        ],
        table: None,
        decor: None,
    };
    let wrap = WrapMap::new(6).sync(content, &[line0]);

    let mut full_rendered_text = String::new();
    let mut buffer_ranges: Vec<(usize, usize)> = Vec::new();
    for seg in wrap.segments().iter().filter(|s| s.model_line == 0) {
        for sp in &seg.spans {
            let text = sp.text(content);
            assert!(
                text.chars().map(control_aware_width).sum::<usize>() <= 6
                    || text.chars().count() == 1,
                "segment exceeds width 6 without being a single over-wide char: {text:?}",
            );
            if sp.is_rendered() {
                full_rendered_text.push_str(text);
                let r = sp.range();
                buffer_ranges.push((r.start, r.end));
            }
        }
    }
    // The concealed inline-code content is split across multiple
    // segments (width 6 can't fit all 20 'a's on one row) but
    // reconstructs exactly, and every piece's buffer range is the
    // SAME full original span range (never narrowed).
    assert_eq!(full_rendered_text, "aaaaaaaaaaaaaaaaaaaa");
    assert!(
        buffer_ranges.len() > 1,
        "expected the rendered span to be split across more than one segment"
    );
    let first = buffer_ranges[0];
    for r in &buffer_ranges {
        assert_eq!(
            *r, first,
            "the span's range must stay at the full original range on every slice"
        );
    }
}

/// `CELL-ORDER` regression: a `Substituted` span (a
/// concealed link's visible text, same shape `rune-md`'s emitter
/// produces) immediately followed by an `Identical` span whose text
/// starts with a LONE zero-width joiner — exactly what both
/// the two checked-in replay repros reduce to (a ZWJ family
/// emoji pasted right after concealed/marker text, then edited until
/// the emoji's own leading base codepoint is gone, leaving the visible
/// text starting on a bare ZWJ). The renderer's actual `Cell` layout
/// ALWAYS grapheme-segments each span's own text independently,
/// span by span: the substituted `"a"` never
/// joins to the ZWJ starting the next span, because that span's text
/// is segmented on its own, starting fresh — two single-width cells,
/// not one fused cluster (a lone ZWJ has nothing to join to at the
/// start of a string). `visual_col`/`byte_col_from_visual` must agree,
/// or a cursor's computed column stops lining up with any real `Cell`
/// — the caret placer's "no matching column" fallback then
/// appends a synthetic caret cell at the row's END, out of
/// `buf_offset` order, which is the actual `CELL-ORDER` failure both
/// repros hit.
#[test]
fn visual_col_does_not_fuse_a_zwj_across_a_span_boundary() {
    let content = "a\u{200d}\u{1f469}"; // "a" + ZWJ + 👩
    let spans = vec![
        SyntaxSpan::substituted(0, "a".to_string(), TEXT, 0..1),
        SyntaxSpan::Identical {
            scope: TEXT,
            range: 1..content.len(),
        },
    ];

    // Per-span segmentation (what the renderer actually builds): "a"
    // (width 1), then the SECOND span segmented on its own — its text
    // starts fresh, so the ZWJ has no preceding char to join to and
    // stands as its own cluster (a LONE zero-width rune, ratatui-
    // matching per `grapheme_width`'s doc: width 0) — then the emoji
    // (width 2). Total: 1 + 0 + 2 = 3. A concatenated-then-segmented
    // walk instead fuses "a" and the ZWJ into one cluster (UAX #29 GB9
    // joins a ZWJ to WHATEVER precedes it, unconditionally) and
    // OVERcounts here — the fused "a"+ZWJ cluster measures via
    // `UnicodeWidthStr::width` (1) instead of "a" and the lone ZWJ
    // being counted separately (1 + 0), so fusing and not fusing
    // happen to agree on 1 for THIS pair; what fusing gets wrong is
    // WHICH byte range the width belongs to, not the row total — the
    // `byte_col_from_visual` round-trip below is what actually pins
    // that a span boundary forces the break.
    let end = query::visual_col(content, &spans, content.len());
    assert_eq!(
        end, 3,
        "a span boundary must still force a grapheme-cluster break \
         even before a now-zero-width ZWJ — got {end}"
    );

    // Round-trips: the byte offset for that same visual column is the
    // whole content, never somewhere mid-cluster.
    assert_eq!(
        query::byte_col_from_visual(content, &spans, end),
        content.len()
    );
}
