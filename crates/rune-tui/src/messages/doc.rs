use rune_core::buffer::Buffer;

use super::{EMPTY_TEXT, Message, Severity};
use crate::app::App;
use crate::document::{Document, ReadOnly};

pub(super) fn sanitize(text: &str) -> String {
    // C1 controls (U+0080-U+009F) decode from UTF-8 as ordinary two-byte
    // sequences, unlike C0/DEL which are single-byte, so a filesystem path
    // or error string can carry one without it looking like a raw control
    // byte on the wire.
    text.chars()
        .filter(|&c| {
            c == '\n'
                || !(('\u{0}'..='\u{1f}').contains(&c)
                    || c == '\u{7f}'
                    || ('\u{80}'..='\u{9f}').contains(&c))
        })
        .collect()
}

fn build_markdown(entries: &[Message]) -> (String, Vec<(std::ops::Range<usize>, Severity)>) {
    if entries.is_empty() {
        return (EMPTY_TEXT.to_string(), Vec::new());
    }
    let mut text = String::new();
    let mut ranges = Vec::with_capacity(entries.len());
    for (i, entry) in entries.iter().enumerate() {
        if i > 0 {
            text.push_str("\n\n");
        }
        let start = text.len();
        text.push_str(entry.severity.glyph());
        text.push_str(&entry.text);
        if entry.count.get() > 1 {
            text.push_str(&format!(" x{}", entry.count));
        }
        ranges.push((start..text.len(), entry.severity));
    }
    (text, ranges)
}

pub(super) fn rebuild_doc(app: &mut App) {
    let (text, ranges) = build_markdown(&app.messages.entries);
    let focused = app.messages.doc.focused;
    let scroll_row = app.messages.doc.viewport.scroll_row;
    let width = app.messages.doc.viewport.width;
    let mut doc = Document::new(Buffer::new(text));
    doc.read_only = ReadOnly::Always;
    doc.focused = focused;
    doc.viewport.scroll_row = scroll_row;
    doc.viewport.width = width;
    app.messages.doc = doc;
    app.messages.ranges = ranges;
}
