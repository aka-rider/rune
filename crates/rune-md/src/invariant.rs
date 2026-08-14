//! Byte-accounting invariants over `emit` output: every buffer byte is
//! either visible in exactly one span or accounted for as hidden, and
//! `SyntaxSnapshot`'s coordinate maps stay monotonic and round-trip
//! stable. Gated behind `fuzz-hooks` (and test builds) — see that
//! feature's own docs in `Cargo.toml` — so a consumer that only needs
//! these checks is not forced to also arm `strict-invariants`.
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

/// The invariant BLOCKER 1 violated: per line, the sum of every span's
/// buffer-byte length plus every hidden range's byte length must equal the
/// line's exact byte length. A per-LINE `touched` bool couldn't distinguish
/// a partially-covered line from a fully-covered one; this checks BYTES.
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

#[allow(clippy::needless_range_loop)] // `line` also indexes buf.line()/snap.hidden_byte_count(), not just `lines`
pub fn assert_no_duplicate_content_at(content: &str, cursor_offsets: &[usize], width: u16) {
    for &focused in &[true, false] {
        let (buf, doc) = synced_at(content, cursor_offsets, focused, width);
        let (lines, snap) = emit(buf.content(), doc.blocks(), width);
        assert_full_line_coverage(&buf, &lines, &snap);

        for line in 0..buf.line_count() {
            let line_len = buf.line(line).len();
            let l = &lines[line];

            // No two spans on this line may claim overlapping buffer
            // bytes — the literal "content duplicated" shape.
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

            // When nothing on this line is hidden, the emitted text must
            // equal the exact buffer bytes — not longer (duplicated
            // content) or shorter (dropped content). A rendered table row
            // is the one documented exception (plan architectural decision
            // 6, "Table lines emit no hidden ranges"): it substitutes a
            // wholly different, box-drawn string for the same claimed byte
            // RANGE while hiding nothing, so `hidden_byte_count == 0` no
            // longer implies "text is verbatim" once `l.table` is `Some`
            // (`assert_full_line_coverage` above already covers the byte-
            // accounting side of that same row via `range()`, not `text()`).
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

                // The reported symptom: syntax_to_buffer mapping past the
                // buffer line's own end.
                assert!(
                    bp2.col <= line_len,
                    "syntax_to_buffer mapped past end-of-line (focused={focused}) at line {line} col {col}: bp2={bp2:?} line_len={line_len}"
                );
            }
        }
    }
}
