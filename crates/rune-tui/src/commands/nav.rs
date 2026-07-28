//! Cursor movement, selection, select-all, and Escape-collapse (WP6).
//! Ports Go's navigation commands and `multicursor.escape`.
//!
//! Vertical/page motion and the WP7.S2 viewport-only scroll commands live
//! in the sibling `nav_scroll` module (plan WP7.S7, §1.6: this file was
//! already over the 500-line budget before WP7 added anything).
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
//! sees the post-resize wrap. This mirrors Go's own behavior too: Go's
//! command context is built from `m.syntaxSnap`/`m.wrapSnap`, populated by
//! the MOST RECENT `syncDisplay()` — i.e. reflecting cursor/reveal state
//! from before this keystroke's own movement, exactly what calling `view()`
//! at handler-entry (before this handler updates `cursors`) reproduces
//! here.
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
use rune_core::coords::BufferPoint;
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

/// Unicode-aware generalization of `commands_nav.go:getClass` (plan
/// WP9.S1, recorded as a deliberate divergence in `TODO.md`). Go's
/// original classifies `[A-Za-z0-9_]` as `Word` and every other rune —
/// including every non-ASCII letter (Cyrillic, Greek, CJK ideographs,
/// combining marks, …) — as `Other`, so `⌥←`/`⌥→` stop at every individual
/// non-ASCII character instead of the actual word boundary. That is a
/// defect in the Go original, not a behavior worth porting byte-for-byte:
/// this classifier instead asks `unicode-segmentation`'s UAX #29
/// word-boundary algorithm whether `r`, fused between two ordinary word
/// characters, stays part of one word segment — the same rule that
/// already makes `is_ascii_alphanumeric()` true for `a`-`z`/`0`-`9`, but
/// extended to every script's letters, digits and combining marks, not
/// just ASCII's. Whitespace is likewise generalized to `char::is_whitespace`
/// (a superset of Go's ASCII `' '`/`'\t'`/`'\n'`/`'\r'` check).
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
/// `ExtendNumLet`, joins rather than breaks a run, matching Go's explicit
/// `r == '_'` special case without needing one here).
fn is_word_forming(r: char) -> bool {
    let probe = format!("a{r}a");
    probe.split_word_bounds().count() == 1
}

/// Port of `commands_nav.go:prevRuneOffset`. Simplified relative to Go's
/// `utf8.DecodeLastRuneInString` + `RuneError` fallback: `Buffer::content`
/// is a Rust `String`, a UTF-8-valid-by-construction type, so there is no
/// reachable "invalid encoding" case to recover from — walking back to the
/// nearest char boundary (at most 3 bytes) is the whole algorithm.
pub fn prev_rune_offset(buf: &Buffer, offset: usize) -> usize {
    if offset == 0 {
        return 0;
    }
    let content = buf.content();
    let mut i = offset.min(content.len()).saturating_sub(1);
    while i > 0 && !content.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Port of `commands_nav.go:nextRuneOffset`.
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

/// Port of `commands_nav.go:wordLeftOffset`.
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

/// Port of `commands_nav.go:wordRightOffset`.
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

/// Port of `commands_nav.go:lineStartOffset` — toggles between the line's
/// first non-whitespace column and column 0 (a "smart home").
pub fn line_start_offset(buf: &Buffer, offset: usize) -> usize {
    let bp = buf.offset_to_line_col(offset);
    let line_start = buf.line_col_to_offset(BufferPoint {
        line: bp.line,
        col: 0,
    });

    let mut first_non_ws = line_start;
    while first_non_ws < buf.len() {
        let Some((r, size)) = buf.rune_at(first_non_ws) else {
            break;
        };
        if r == '\n' || (r != ' ' && r != '\t') {
            break;
        }
        first_non_ws += size;
    }

    if offset == first_non_ws {
        line_start
    } else {
        first_non_ws
    }
}

/// Port of `commands_nav.go:lineEndOffset`.
pub fn line_end_offset(buf: &Buffer, offset: usize) -> usize {
    let bp = buf.offset_to_line_col(offset);
    let mut end = buf.line_col_to_offset(BufferPoint {
        line: bp.line,
        col: 0,
    });
    while end < buf.len() {
        let Some((r, size)) = buf.rune_at(end) else {
            break;
        };
        if r == '\n' {
            break;
        }
        end += size;
    }
    end
}

/// The byte range `[line_start, line_end)` of the line containing `offset`,
/// extended to include the line's trailing `\n` unless it's the buffer's
/// last line. The shared chokepoint for "whole current line" ranges: port
/// of `commands_clipboard.go:copyEntireLine`'s range arithmetic, used
/// identically by `commands::clipboard::copy_entire_line` (what gets
/// copied) and `commands::edit::delete_selection_or_line` (what cut
/// removes) so the two can never disagree about where a line-copy ends.
pub(crate) fn line_range_incl_newline(buf: &Buffer, offset: usize) -> (usize, usize) {
    let bp = buf.offset_to_line_col(offset);
    let line_start = buf.line_start(bp.line);
    let mut line_end = buf.line_end(bp.line);
    if line_end < buf.len() {
        line_end += 1; // include the trailing '\n'
    }
    (line_start, line_end)
}

/// The `[start, end)` byte range of the word (or whitespace/punctuation
/// run) touching `offset` — the double-click "select word" gesture
/// (`commands::mouse`, plan WP7.S6). Class-based like `word_left_offset`/
/// `word_right_offset` above, but expands outward from a single anchor
/// rather than walking motion-by-motion, since a click can land anywhere
/// inside the run, not just at its start. `line_range_incl_newline` above is
/// the equivalent chokepoint for the triple-click "select the whole
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

/// Port of `commands_nav.go:selectionEndInclusive`. Used both by movement
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

/// Port of `commands_nav.go:updateHorizontal`: recomputes `desired_col`
/// from the NEW position's visual column — every horizontal/line-start-end
/// motion resets `desired_col` this way (only vertical row motion preserves
/// the caller's `desired_col`, see `move_row` below).
fn update_horizontal(
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

/// Port of `commands_nav.go:handleLeftCmd`.
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

/// Port of `commands_nav.go:handleRightCmd`.
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

/// Port of `commands_nav.go:handleMoveTo`.
fn handle_move_to(
    view: &ViewSnapshots,
    buf: &Buffer,
    c: Cursor,
    select: bool,
    step: impl Fn(&Buffer, usize) -> usize,
) -> Cursor {
    let offset = step(buf, c.position);
    update_horizontal(view, buf, c, offset, select)
}

/// Shared horizontal/line-start-end driver — port of `handleCursorCmd`
/// applied to every cursor in the set.
fn move_cursors(
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

pub fn line_start(doc: &mut Document, select: bool) {
    move_cursors(doc, select, |view, buf, c, select| {
        handle_move_to(view, buf, c, select, line_start_offset)
    });
}

pub fn line_end(doc: &mut Document, select: bool) {
    move_cursors(doc, select, |view, buf, c, select| {
        handle_move_to(view, buf, c, select, line_end_offset)
    });
}

/// Port of `commands_nav_gen.go:execSelectAll`.
pub fn select_all(doc: &mut Document) {
    let all = doc.cursors.all();
    let mut c = all.first().copied().unwrap_or_default();
    c.position = doc.buffer.len();
    c.anchor = 0;
    c.desired_col = 0;
    doc.cursors = CursorSet::new_from(&[c]);
}

/// Port of `commands_multi.go:execMulticursorEscape` — the Escape
/// hardcoded fast path (plan Context, "Hardcoded fast paths outside the
/// resolver"): multi-cursor collapses to the primary; a single cursor with
/// a selection collapses the selection; otherwise a no-op.
pub fn escape(doc: &mut Document) {
    if doc.cursors.is_multi() {
        let primary = doc.cursors.primary();
        doc.cursors = doc.cursors.collapse_to(primary);
        return;
    }
    let primary = doc.cursors.primary();
    if primary.has_selection() {
        doc.cursors = CursorSet::new_from(&[primary.collapse_to_position()]);
    }
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
        // Starting mid-whitespace, Go's wordRightOffset skips the
        // whitespace run AND the following word class run in the same
        // call (commands_nav.go:107-136) — it does not stop at the start
        // of "world".
        assert_eq!(word_right_offset(&buf, 5), 13);
        // Starting mid-word only skips to the end of the CURRENT word,
        // stopping at the following whitespace run.
        assert_eq!(word_right_offset(&buf, 2), 5);
    }

    /// Regression for WP9.S1 (the Go word-classification defect): a
    /// non-ASCII alphabet must still form one word run, so `⌥→`/`⌥←` stop
    /// at the WORD boundary, never at every individual Cyrillic character.
    /// Under Go's original ASCII-only classifier every non-ASCII letter is
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

    /// `_` still joins a word run (Go's explicit `r == '_'` special case),
    /// and ASCII digits/letters keep classifying together with a
    /// following Unicode letter (mixed identifiers stay one word).
    #[test]
    fn underscore_and_mixed_ascii_unicode_runs_stay_one_word() {
        let buf = Buffer::new("foo_bar привіт1");
        assert_eq!(word_right_offset(&buf, 0), "foo_bar".len());
        assert_eq!(word_right_offset(&buf, "foo_bar ".len()), buf.len());
    }

    #[test]
    fn line_start_offset_toggles_first_non_ws_and_column_zero() {
        let buf = Buffer::new("   indented\n");
        // Cursor already at the first non-whitespace column: toggling goes
        // to column 0.
        assert_eq!(line_start_offset(&buf, 3), 0);
        // Cursor elsewhere on the line: goes to the first non-whitespace
        // column.
        assert_eq!(line_start_offset(&buf, 7), 3);
    }

    #[test]
    fn line_end_offset_stops_before_the_newline() {
        let buf = Buffer::new("hello\nworld\n");
        assert_eq!(line_end_offset(&buf, 0), 5);
        assert_eq!(line_end_offset(&buf, 3), 5);
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
