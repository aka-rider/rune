#![allow(clippy::indexing_slicing)]

use rune_core::buffer::Buffer;
use rune_core::coords::BufferPoint;
use rune_core::cursor::CursorSet;
use rune_syntax::{SyntaxLine, SyntaxSnapshot};

use crate::element::doc::DocMachine;
use crate::emit::emit;

fn synced_at(
    content: &str,
    cursor_offsets: &[usize],
    focused: bool,
    width: u16,
) -> (Buffer, DocMachine) {
    let buf = Buffer::new(content);
    let mut doc = DocMachine::new();
    doc.set_width(width);
    doc.set_reveal_mode(focused.into());
    doc.sync_content(&buf);
    let positions: Vec<usize> = cursor_offsets.iter().map(|&o| o.min(buf.len())).collect();
    let cursors = CursorSet::new_from_positions(&positions);
    doc.sync_cursors(&buf, &cursors);
    (buf, doc)
}

pub fn assert_full_line_coverage(buf: &Buffer, lines: &[SyntaxLine], snap: &SyntaxSnapshot) {
    for line in 0..buf.line_count() {
        let expected_len = buf.line(line).len();
        let visible: usize = lines.get(line).map_or(0, |l| {
            l.spans
                .iter()
                .map(|s| {
                    let r = s.range();
                    r.end.saturating_sub(r.start)
                })
                .sum()
        });
        let hidden = snap.hidden_byte_count(line);
        assert_eq!(
            visible + hidden,
            expected_len,
            "line {line} ({:?}): visible {visible} + hidden {hidden} != line length {expected_len} — a byte was silently dropped",
            buf.line(line)
        );
    }
}

pub fn assert_no_duplicate_content(content: &str) {
    assert_no_duplicate_content_at(content, &[0], 80);
}

#[allow(clippy::needless_range_loop)]
pub fn assert_no_duplicate_content_at(content: &str, cursor_offsets: &[usize], width: u16) {
    for &focused in &[true, false] {
        let (buf, doc) = synced_at(content, cursor_offsets, focused, width);
        let (lines, snap) = emit(buf.content(), doc.blocks(), width);
        assert_full_line_coverage(&buf, &lines, &snap);

        for line in 0..buf.line_count() {
            let line_len = buf.line(line).len();
            let l = &lines[line];

            for i in 0..l.spans.len() {
                for j in (i + 1)..l.spans.len() {
                    let a = &l.spans[i];
                    let b = &l.spans[j];
                    let (ar, br) = (a.range(), b.range());
                    assert!(
                        ar.end <= br.start || br.end <= ar.start,
                        "line {line} (focused={focused}): spans {i} {a:?} and {j} {b:?} claim overlapping buffer bytes"
                    );
                }
            }

            if snap.hidden_byte_count(line) == 0 && l.table.is_none() {
                let joined: String = l.spans.iter().map(|s| s.text(content)).collect();
                assert_eq!(
                    joined,
                    buf.line(line),
                    "line {line} (focused={focused}): rendered text != exact buffer bytes"
                );
            }

            let mut prev_syntax_col = None;
            for col in 0..=line_len {
                let bp = BufferPoint { line, col };
                let sp = snap.buffer_to_syntax(bp);
                if let Some(prev) = prev_syntax_col {
                    assert!(
                        sp.col >= prev,
                        "buffer_to_syntax not monotonic (focused={focused}) at line {line} col {col}: prev={prev} now={}",
                        sp.col
                    );
                }
                prev_syntax_col = Some(sp.col);

                let bp2 = snap.syntax_to_buffer(sp);
                let sp2 = snap.buffer_to_syntax(bp2);
                assert_eq!(
                    sp, sp2,
                    "round-trip stability failed (focused={focused}) at line {line} col {col}: bp={bp:?} sp={sp:?} bp2={bp2:?} sp2={sp2:?}"
                );

                assert!(
                    bp2.col <= line_len,
                    "syntax_to_buffer mapped past end-of-line (focused={focused}) at line {line} col {col}: bp2={bp2:?} line_len={line_len}"
                );
            }
        }
    }
}
