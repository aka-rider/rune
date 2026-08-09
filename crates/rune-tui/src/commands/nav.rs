//! Cursor movement, selection, select-all, and Escape-collapse (WP6).
//!
//! Vertical/page motion and the WP7.S2 viewport-only scroll commands live
//! in the sibling `nav_scroll` module (plan WP7.S7: this file was
//! already over the 500-line budget before WP7 added anything). Line/
//! document motion (line start/end, and the `handle_move_to` driver) lives
//! in the sibling `nav_line` module for the same reason; that module
//! reaches back into this one for the shared `move_cursors`/
//! `update_horizontal` cursor-stepping infrastructure.
//!
//! Doc-local (plan WP1 decision 4): every function here takes `&mut
//! Document` directly — motion/selection never touches `App`-level state
//! (the recovery store, status message, dirty cache), so there is no reason
//! to thread a `DocumentId` through this module at all.
//!
//! Every handler that needs Buffer<->Syntax<->Wrap coordinate conversions
//! calls `Document::view()` fresh at entry rather than reading `Document::
//! view` (the cached field): that cache is only refreshed once per whole
//! message BATCH (by the runtime, after every `Msg` in the batch has been
//! applied — see `runtime::run`), so within a batch it can still reflect
//! the state from BEFORE an earlier `Msg::Resize` in the same batch already
//! widened the viewport. `Document::view()` is documented idempotent/cheap
//! and always reflects the CURRENT `Document` fields (`viewport.width` in
//! particular), so a `Key` handled right after a `Resize` in the same batch
//! sees the post-resize wrap. That is: `view()` called at handler-entry,
//! before this handler updates `cursors`, reflects cursor/reveal state from
//! before this keystroke's own movement.
//!
//! Handlers here call `view()`, NEVER `sync()` (review finding F4):
//! `sync()` also scrolls the viewport toward the PRIMARY cursor, and this
//! module calls in BEFORE it has updated `cursors` for this motion — an
//! intermediate scroll toward a soon-to-change cursor that the batch's
//! real settle (`App::sync_view`, called once per batch) would then
//! silently overwrite. `viewport.scroll_row` has exactly one writer:
//! `Document::scroll_to_cursor`, invoked exactly once per settled batch via
//! `Document::sync`/`App::sync_view` — never from inside a single command.

use rune_core::buffer::Buffer;
use rune_core::cursor::{Cursor, CursorSet};
use rune_md::element::doc::ViewSnapshots;
use unicode_segmentation::UnicodeSegmentation;

use crate::document::Document;

#[derive(Clone, Copy, PartialEq, Eq)]
enum CharClass {
    Whitespace,
    Word,
    Other,
}

/// Unicode-aware word classifier (plan WP9.S1, recorded as a deliberate
/// divergence in `TODO.md`): asks `unicode-segmentation`'s UAX #29
/// word-boundary algorithm whether `r`, fused between two ordinary word
/// characters, stays part of one word segment — the same rule that
/// already makes `is_ascii_alphanumeric()` true for `a`-`z`/`0`-`9`, but
/// extended to every script's letters, digits and combining marks, not
/// just ASCII's. A naive ASCII-only classifier would treat every non-ASCII
/// letter (Cyrillic, Greek, CJK ideographs, combining marks, …) as `Other`,
/// so `⌥←`/`⌥→` would stop at every individual character instead of the
/// actual word boundary. Whitespace is likewise generalized to
/// `char::is_whitespace`.
fn char_class(r: char) -> CharClass {
    if r.is_whitespace() {
        CharClass::Whitespace
    } else if is_word_forming(r) {
        CharClass::Word
    } else {
        CharClass::Other
    }
}

/// Probes `unicode-segmentation`'s word-boundary algorithm: wraps `r`
/// between two ASCII letters and asks whether the three stay fused into a
/// single UAX #29 word segment. The crate exposes no direct per-character
/// word-break property, so this is the public API's own way to classify
/// one character — it mirrors `unicode_words()`'s own definition of a
/// word: a maximal run of Alphabetic/Numeric runes and the marks/joiners
/// that combine with them (which is also why `_`, Unicode's
/// `ExtendNumLet`, joins rather than breaks a run).
fn is_word_forming(r: char) -> bool {
    let probe = format!("a{r}a");
    probe.split_word_bounds().count() == 1
}

/// `Buffer::content` is a Rust `String`, a UTF-8-valid-by-construction type,
/// so there is no reachable "invalid encoding" case to recover from —
/// walking back to the nearest char boundary (at most 3 bytes) is the whole
/// algorithm.
pub fn prev_rune_offset(buf: &Buffer, offset: usize) -> usize {
    if offset == 0 {
        return 0;
    }
    let content = buf.content();
    content.floor_char_boundary(offset.min(content.len()).saturating_sub(1))
}

pub fn next_rune_offset(buf: &Buffer, offset: usize) -> usize {
    if offset >= buf.len() {
        return buf.len();
    }
    match buf.rune_at(offset) {
        Some((_, size)) => offset + size,
        None => offset + 1,
    }
}

fn class_at(buf: &Buffer, offset: usize) -> CharClass {
    buf.rune_at(offset)
        .map(|(r, _)| char_class(r))
        .unwrap_or(CharClass::Other)
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

/// The `[start, end)` byte range of the word (or whitespace/punctuation
/// run) touching `offset` — the double-click "select word" gesture
/// (`commands::mouse`, plan WP7.S6). Class-based like `word_left_offset`/
/// `word_right_offset` above, but expands outward from a single anchor
/// rather than walking motion-by-motion, since a click can land anywhere
/// inside the run, not just at its start. `nav_line::line_range_incl_newline`
/// is the equivalent chokepoint for the triple-click "select the whole
/// logical line" gesture — it already spans every wrapped row of the
/// buffer line, since it works in buffer-line space, not wrap-row space.
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

/// Used both by movement
/// (implicitly, via `handle_left`/`handle_right`'s `SelectionStart`/`End`)
/// and by `commands::edit`'s selection-replacing edits.
pub fn selection_end_inclusive(c: &Cursor, buf: &Buffer) -> usize {
    let mut end = c.selection_end();
    if c.reversed()
        && end < buf.len()
        && let Some((r, _)) = buf.rune_at(end)
        && r != '\n'
    {
        end = next_rune_offset(buf, end);
    }
    end
}

/// Recomputes `desired_col`
/// from the NEW position's visual column — every horizontal/line-start-end
/// motion resets `desired_col` this way (only vertical row motion preserves
/// the caller's `desired_col`, see `move_row` below).
pub(crate) fn update_horizontal(
    view: &ViewSnapshots,
    buf: &Buffer,
    c: Cursor,
    offset: usize,
    select: bool,
) -> Cursor {
    let bp = buf.offset_to_line_col(offset);
    let sp = view.syntax.buffer_to_syntax(bp);
    let wp = view.wrap.syntax_to_wrap(sp);
    let desired_col = view.wrap.visual_col(buf.content(), wp.row, wp.col);
    Cursor {
        position: offset,
        anchor: if select { c.anchor } else { offset },
        desired_col,
        id: c.id,
    }
}

fn handle_left(
    view: &ViewSnapshots,
    buf: &Buffer,
    c: Cursor,
    select: bool,
    step: impl Fn(&Buffer, usize) -> usize,
) -> Cursor {
    let mut offset = step(buf, c.position);
    if !select && c.has_selection() {
        offset = c.selection_start();
    }
    update_horizontal(view, buf, c, offset, select)
}

fn handle_right(
    view: &ViewSnapshots,
    buf: &Buffer,
    c: Cursor,
    select: bool,
    step: impl Fn(&Buffer, usize) -> usize,
) -> Cursor {
    let mut offset = step(buf, c.position);
    if !select && c.has_selection() {
        offset = c.selection_end();
    }
    update_horizontal(view, buf, c, offset, select)
}

/// Shared horizontal/line-start-end driver, applied to every cursor in the
/// set.
pub(crate) fn move_cursors(
    doc: &mut Document,
    select: bool,
    step: impl Fn(&ViewSnapshots, &Buffer, Cursor, bool) -> Cursor,
) {
    let view = doc.view();
    let new_cursors: Vec<Cursor> = doc
        .cursors
        .all()
        .into_iter()
        .map(|c| step(&view, &doc.buffer, c, select))
        .collect();
    doc.cursors = CursorSet::new_from(&new_cursors);
}

pub fn char_left(doc: &mut Document, select: bool) {
    move_cursors(doc, select, |view, buf, c, select| {
        handle_left(view, buf, c, select, prev_rune_offset)
    });
}

pub fn char_right(doc: &mut Document, select: bool) {
    move_cursors(doc, select, |view, buf, c, select| {
        handle_right(view, buf, c, select, next_rune_offset)
    });
}

pub fn word_left(doc: &mut Document, select: bool) {
    move_cursors(doc, select, |view, buf, c, select| {
        handle_left(view, buf, c, select, word_left_offset)
    });
}

pub fn word_right(doc: &mut Document, select: bool) {
    move_cursors(doc, select, |view, buf, c, select| {
        handle_right(view, buf, c, select, word_right_offset)
    });
}

pub fn select_all(doc: &mut Document) {
    let all = doc.cursors.all();
    let mut c = all.first().copied().unwrap_or_default();
    c.position = doc.buffer.len();
    c.anchor = 0;
    c.desired_col = 0;
    doc.cursors = CursorSet::new_from(&[c]);
}

/// Whether `escape` below found something in the buffer to collapse, or
/// left it untouched — the cascade's own verdict, reported so the caller
/// (`dispatch::handle_editor_key`'s hardcoded Escape fast path) knows
/// whether to keep going: multi-cursor and selection collapse stay in the
/// editor, `Unconsumed` is the cue to leave for the Explorer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EscapeOutcome {
    Collapsed,
    Unconsumed,
}

/// The Escape
/// hardcoded fast path (plan Context, "Hardcoded fast paths outside the
/// resolver"): multi-cursor collapses to the primary; a single cursor with
/// a selection collapses the selection; otherwise reports `Unconsumed` so
/// the cascade can fall through to leaving the editor.
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

    #[test]
    fn prev_next_rune_offset_never_split_a_multibyte_char() {
        let buf = Buffer::new("a\u{6c49}b"); // 'a', 汉 (3 bytes), 'b'
        let after_kanji = 1 + '\u{6c49}'.len_utf8();
        assert_eq!(next_rune_offset(&buf, 1), after_kanji);
        assert_eq!(prev_rune_offset(&buf, after_kanji), 1);
    }

    #[test]
    fn word_left_right_skip_whole_words_and_whitespace_runs() {
        let buf = Buffer::new("hello   world");
        assert_eq!(word_left_offset(&buf, 13), 8); // start of "world"
        assert_eq!(word_right_offset(&buf, 0), 5); // end of "hello"
        // Starting mid-whitespace, this skips the
        // whitespace run AND the following word class run in the same
        // call — it does not stop at the start
        // of "world".
        assert_eq!(word_right_offset(&buf, 5), 13);
        // Starting mid-word only skips to the end of the CURRENT word,
        // stopping at the following whitespace run.
        assert_eq!(word_right_offset(&buf, 2), 5);
    }

    /// Regression for WP9.S1: a
    /// non-ASCII alphabet must still form one word run, so `⌥→`/`⌥←` stop
    /// at the WORD boundary, never at every individual Cyrillic character.
    /// A naive ASCII-only classifier would treat every non-ASCII letter as
    /// `Other`, so `word_right_offset` from 0 would stop after a single
    /// rune instead of at the end of "привіт".
    #[test]
    fn word_motion_treats_a_non_ascii_alphabet_as_one_word() {
        let buf = Buffer::new("привіт світ");
        let privit_end = "привіт".len();
        let svit_start = "привіт ".len();
        assert_eq!(word_right_offset(&buf, 0), privit_end);
        assert_eq!(word_left_offset(&buf, buf.len()), svit_start);
    }

    /// `_` still joins a word run, and ASCII digits/letters keep classifying together with a
    /// following Unicode letter (mixed identifiers stay one word).
    #[test]
    fn underscore_and_mixed_ascii_unicode_runs_stay_one_word() {
        let buf = Buffer::new("foo_bar привіт1");
        assert_eq!(word_right_offset(&buf, 0), "foo_bar".len());
        assert_eq!(word_right_offset(&buf, "foo_bar ".len()), buf.len());
    }

    #[test]
    fn selection_end_inclusive_advances_past_reversed_anchor_unless_newline() {
        let buf = Buffer::new("hello\nworld");
        let reversed = Cursor {
            position: 0,
            anchor: 5,
            desired_col: 0,
            id: 1,
        };
        // Anchor byte is '\n' at offset 5: must NOT advance past it.
        assert_eq!(selection_end_inclusive(&reversed, &buf), 5);

        let reversed2 = Cursor {
            position: 0,
            anchor: 4,
            desired_col: 0,
            id: 1,
        };
        // Anchor byte is 'o' (not '\n'): advances one rune past it.
        assert_eq!(selection_end_inclusive(&reversed2, &buf), 5);
    }
}
