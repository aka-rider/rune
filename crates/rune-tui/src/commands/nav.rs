use rune_core::bracket::{bracket_pair, jump_origin};
use rune_core::buffer::Buffer;
use rune_core::coords::{BufferOffset, VisualCol};
use rune_core::cursor::{Cursor, CursorSet};
use rune_md::element::doc::ViewSnapshots;
use unicode_segmentation::UnicodeSegmentation;

use crate::document::Document;
use crate::keymap::Extend;

#[derive(Clone, Copy, PartialEq, Eq)]
enum CharClass {
    Whitespace,
    Word,
    Other,
}

fn char_class(r: char) -> CharClass {
    if r.is_whitespace() {
        CharClass::Whitespace
    } else if is_word_forming(r) {
        CharClass::Word
    } else {
        CharClass::Other
    }
}

/// `unicode-segmentation` exposes no direct per-character word-break
/// property, so this probes it indirectly: wrap `r` between two ASCII
/// letters and ask whether the three stay fused into a single UAX #29 word
/// segment.
fn is_word_forming(r: char) -> bool {
    let probe = format!("a{r}a");
    probe.split_word_bounds().count() == 1
}

pub fn prev_rune_offset(buf: &Buffer, offset: usize) -> usize {
    if offset == 0 {
        return 0;
    }
    let content = buf.content();
    let candidate = content.floor_char_boundary(offset.min(content.len()).saturating_sub(1));
    match buf.crlf_pair_at(candidate) {
        Some(pair) if pair.end == candidate + 1 => pair.start,
        _ => candidate,
    }
}

pub fn next_rune_offset(buf: &Buffer, offset: usize) -> usize {
    if offset >= buf.len() {
        return buf.len();
    }
    if let Some(pair) = buf.crlf_pair_at(offset)
        && pair.start == offset
    {
        return pair.end;
    }
    match buf.rune_at(offset) {
        Some((_, size)) => offset + size,
        None => offset + 1,
    }
}

fn class_at(buf: &Buffer, offset: usize) -> CharClass {
    buf.rune_at(offset)
        .map_or(CharClass::Other, |(r, _)| char_class(r))
}

pub fn word_left_offset(buf: &Buffer, offset: usize) -> usize {
    if offset == 0 {
        return 0;
    }
    let mut offset = prev_rune_offset(buf, offset);
    let mut start_class = class_at(buf, offset);

    if start_class == CharClass::Whitespace {
        while offset > 0 {
            let prev = prev_rune_offset(buf, offset);
            if class_at(buf, prev) != CharClass::Whitespace {
                break;
            }
            offset = prev;
        }
        if offset == 0 {
            return 0;
        }
        offset = prev_rune_offset(buf, offset);
        start_class = class_at(buf, offset);
    }

    while offset > 0 {
        let prev = prev_rune_offset(buf, offset);
        if class_at(buf, prev) != start_class {
            break;
        }
        offset = prev;
    }
    offset
}

pub fn word_right_offset(buf: &Buffer, offset: usize) -> usize {
    if offset >= buf.len() {
        return offset;
    }
    let mut offset = offset;
    let start_class = class_at(buf, offset);

    while offset < buf.len() {
        let Some((r, size)) = buf.rune_at(offset) else {
            break;
        };
        if char_class(r) != start_class {
            break;
        }
        offset += size;
    }

    if start_class == CharClass::Whitespace && offset < buf.len() {
        let next_class = class_at(buf, offset);
        while offset < buf.len() {
            let Some((r, size)) = buf.rune_at(offset) else {
                break;
            };
            if char_class(r) != next_class {
                break;
            }
            offset += size;
        }
    }
    offset
}

pub(crate) fn is_word_at(buf: &Buffer, offset: usize) -> bool {
    if buf.is_empty() {
        return false;
    }
    let anchor = if offset < buf.len() {
        offset
    } else {
        prev_rune_offset(buf, offset)
    };
    class_at(buf, anchor) == CharClass::Word
}

pub(crate) fn word_range_at(buf: &Buffer, offset: usize) -> (usize, usize) {
    if buf.is_empty() {
        return (0, 0);
    }
    // A click at (or past) EOF anchors on the last real byte instead of an
    // out-of-range class lookup.
    let anchor = if offset < buf.len() {
        offset
    } else {
        prev_rune_offset(buf, offset)
    };
    let class = class_at(buf, anchor);

    let mut start = anchor;
    while start > 0 {
        let prev = prev_rune_offset(buf, start);
        if class_at(buf, prev) != class {
            break;
        }
        start = prev;
    }

    let mut end = anchor;
    while end < buf.len() && class_at(buf, end) == class {
        end = next_rune_offset(buf, end);
    }

    (start, end)
}

pub(crate) fn update_horizontal(
    view: &ViewSnapshots,
    buf: &Buffer,
    c: Cursor,
    offset: usize,
    extend: Extend,
) -> Cursor {
    let bp = buf.offset_to_line_col(offset);
    let sp = view.syntax.buffer_to_syntax(bp);
    let wp = view.wrap.syntax_to_wrap(sp);
    let desired_col = view.wrap.visual_col(buf.content(), wp.row, wp.col);
    Cursor {
        position: BufferOffset(offset),
        anchor: if extend == Extend::Yes {
            c.anchor
        } else {
            BufferOffset(offset)
        },
        desired_col: VisualCol(desired_col),
        id: c.id,
    }
}

fn handle_left(
    view: &ViewSnapshots,
    buf: &Buffer,
    c: Cursor,
    extend: Extend,
    step: impl Fn(&Buffer, usize) -> usize,
) -> Cursor {
    let mut offset = step(buf, c.position.get());
    if extend == Extend::No && c.has_selection() {
        offset = c.selection_start().get();
    }
    update_horizontal(view, buf, c, offset, extend)
}

fn handle_right(
    view: &ViewSnapshots,
    buf: &Buffer,
    c: Cursor,
    extend: Extend,
    step: impl Fn(&Buffer, usize) -> usize,
) -> Cursor {
    let mut offset = step(buf, c.position.get());
    if extend == Extend::No && c.has_selection() {
        offset = c.selection_end().get();
    }
    update_horizontal(view, buf, c, offset, extend)
}

/// Reads a fresh `Document::view()` here, not `sync()`: `sync()` also
/// scrolls the viewport toward the primary cursor, and this runs before
/// `cursors` holds this motion's result, so an early scroll would chase a
/// cursor about to move and get overwritten once the batch settles.
pub(crate) fn move_cursors(
    doc: &mut Document,
    extend: Extend,
    step: impl Fn(&ViewSnapshots, &Buffer, Cursor, Extend) -> Cursor,
) {
    let view = doc.view();
    let new_cursors: Vec<Cursor> = doc
        .cursors
        .all()
        .iter()
        .map(|&c| step(&view, &doc.buffer, c, extend))
        .collect();
    doc.cursors = CursorSet::new_from(&new_cursors);
}

pub fn char_left(doc: &mut Document, extend: Extend) {
    move_cursors(doc, extend, |view, buf, c, extend| {
        handle_left(view, buf, c, extend, prev_rune_offset)
    });
}

pub fn char_right(doc: &mut Document, extend: Extend) {
    move_cursors(doc, extend, |view, buf, c, extend| {
        handle_right(view, buf, c, extend, next_rune_offset)
    });
}

pub fn word_left(doc: &mut Document, extend: Extend) {
    move_cursors(doc, extend, |view, buf, c, extend| {
        handle_left(view, buf, c, extend, word_left_offset)
    });
}

pub fn word_right(doc: &mut Document, extend: Extend) {
    move_cursors(doc, extend, |view, buf, c, extend| {
        handle_right(view, buf, c, extend, word_right_offset)
    });
}

pub fn match_bracket(doc: &mut Document, extend: Extend) {
    move_cursors(doc, extend, |view, buf, c, extend| {
        let text = buf.content();
        let Some(origin) = jump_origin(text, c.position.get()) else {
            return c;
        };
        let Some((open, close)) = bracket_pair(text, origin) else {
            return c;
        };
        let target = if open == origin { close } else { open };
        update_horizontal(view, buf, c, target, extend)
    });
}

pub fn select_all(doc: &mut Document) {
    let mut c = doc.cursors.primary();
    c.position = BufferOffset(doc.buffer.len());
    c.anchor = BufferOffset(0);
    c.desired_col = VisualCol(0);
    doc.cursors = CursorSet::new_from(&[c]);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EscapeOutcome {
    Collapsed,
    Unconsumed,
}

pub fn escape(doc: &mut Document) -> EscapeOutcome {
    if doc.cursors.is_multi() {
        let primary = doc.cursors.primary();
        doc.cursors = doc.cursors.collapse_to(primary);
        return EscapeOutcome::Collapsed;
    }
    let primary = doc.cursors.primary();
    if primary.has_selection() {
        doc.cursors = CursorSet::new_from(&[primary.collapse_to_position()]);
        return EscapeOutcome::Collapsed;
    }
    EscapeOutcome::Unconsumed
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use rune_core::cursor::CursorId;

    #[test]
    fn prev_next_rune_offset_never_split_a_multibyte_char() {
        let buf = Buffer::new("a\u{6c49}b");
        let after_kanji = 1 + '\u{6c49}'.len_utf8();
        assert_eq!(next_rune_offset(&buf, 1), after_kanji);
        assert_eq!(prev_rune_offset(&buf, after_kanji), 1);
    }

    #[test]
    fn right_arrow_steps_over_a_crlf_pair_as_one_boundary() {
        let buf = Buffer::new("abc\r\ndef");
        assert_eq!(next_rune_offset(&buf, 3), 5);
    }

    #[test]
    fn left_arrow_steps_back_over_a_crlf_pair_as_one_boundary() {
        let buf = Buffer::new("abc\r\ndef");
        assert_eq!(prev_rune_offset(&buf, 5), 3);
    }

    #[test]
    fn rune_step_does_not_treat_a_bare_cr_as_part_of_a_pair() {
        let buf = Buffer::new("a\rb");
        assert_eq!(next_rune_offset(&buf, 1), 2);
        assert_eq!(prev_rune_offset(&buf, 2), 1);
    }

    #[test]
    fn word_left_right_skip_whole_words_and_whitespace_runs() {
        let buf = Buffer::new("hello   world");
        assert_eq!(word_left_offset(&buf, 13), 8);
        assert_eq!(word_right_offset(&buf, 0), 5);
        assert_eq!(word_right_offset(&buf, 5), 13);
        assert_eq!(word_right_offset(&buf, 2), 5);
    }

    #[test]
    fn word_motion_treats_a_non_ascii_alphabet_as_one_word() {
        let buf = Buffer::new("привіт світ");
        let privit_end = "привіт".len();
        let svit_start = "привіт ".len();
        assert_eq!(word_right_offset(&buf, 0), privit_end);
        assert_eq!(word_left_offset(&buf, buf.len()), svit_start);
    }

    #[test]
    fn underscore_and_mixed_ascii_unicode_runs_stay_one_word() {
        let buf = Buffer::new("foo_bar привіт1");
        assert_eq!(word_right_offset(&buf, 0), "foo_bar".len());
        assert_eq!(word_right_offset(&buf, "foo_bar ".len()), buf.len());
    }

    fn doc_with(content: &str, anchor: usize, position: usize) -> Document {
        let mut doc = Document::new(Buffer::new(content));
        doc.cursors = CursorSet::new_from(&[Cursor {
            position: BufferOffset(position),
            anchor: BufferOffset(anchor),
            desired_col: VisualCol(0),
            id: CursorId::FIRST,
        }]);
        doc.viewport.set_size(80, 23);
        doc
    }

    #[test]
    fn left_on_a_reversed_selection_collapses_to_its_low_edge() {
        let mut doc = doc_with("hello world", 5, 2);
        char_left(&mut doc, Extend::No);
        assert_eq!(doc.cursors.primary().position, BufferOffset(2));
    }

    #[test]
    fn left_on_a_forward_selection_collapses_to_its_low_edge() {
        let mut doc = doc_with("hello world", 2, 5);
        char_left(&mut doc, Extend::No);
        assert_eq!(doc.cursors.primary().position, BufferOffset(2));
    }

    #[test]
    fn right_on_a_reversed_selection_collapses_to_its_high_edge() {
        let mut doc = doc_with("hello world", 5, 0);
        char_right(&mut doc, Extend::No);
        assert_eq!(doc.cursors.primary().position, BufferOffset(5));
    }

    #[test]
    fn right_on_a_forward_selection_collapses_to_its_high_edge() {
        let mut doc = doc_with("hello world", 0, 5);
        char_right(&mut doc, Extend::No);
        assert_eq!(doc.cursors.primary().position, BufferOffset(5));
    }
}
