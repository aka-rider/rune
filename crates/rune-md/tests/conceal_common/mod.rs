//! Shared fixtures for the `conceal_roundtrip_*` sibling test files: build a
//! synced `(Buffer, DocMachine)` pair and join a rendered line back to a
//! `String`. The byte-accounting assertion helpers this split reuses live
//! in `rune_md::invariant` — every producer bug they catch is exactly as
//! reachable from that crate's own test suite as from here.
//! `#![allow(dead_code)]` because each consumer binary only calls a subset
//! of these — the rest would otherwise trip `-D warnings`' dead-code lint
//! in that particular binary.
#![allow(dead_code)]

use rune_core::buffer::Buffer;
use rune_core::cursor::CursorSet;
use rune_md::element::doc::DocMachine;

pub fn synced(content: &str, cursor_offset: usize, focused: bool) -> (Buffer, DocMachine) {
    synced_at(content, &[cursor_offset], focused, 0)
}

pub fn synced_at(
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

pub fn joined_line(lines: &[rune_syntax::SyntaxLine], line: usize, content: &str) -> String {
    lines
        .get(line)
        .map(|l| l.spans.iter().map(|s| s.text(content)).collect::<String>())
        .unwrap_or_default()
}
