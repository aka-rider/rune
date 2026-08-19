//! The shared editing core behind every hand-rolled single-line query
//! buffer — `SearchState.draft`, `FileSearchState.query`, and
//! `Overlay::ExplorerFind`'s `String`. None of the three needs cursor
//! placement, selection, or undo (`field::TextField`'s own job, for
//! `TitleField`/`PaletteState`): each is append-only from the keyboard and
//! only ever erases from its own end, so a plain `String` plus these three
//! free functions is the whole shared surface.

use unicode_segmentation::UnicodeSegmentation;

pub(crate) fn type_char(text: &mut String, c: char) {
    text.push(c);
}

/// Erases one GRAPHEME CLUSTER, not one `char` — a combining mark popped
/// alone would desync what's on screen from what the buffer holds. A no-op
/// on an already-empty `text`.
pub(crate) fn erase_grapheme(text: &mut String) {
    if let Some((byte_idx, _)) = text.grapheme_indices(true).next_back() {
        text.truncate(byte_idx);
    }
}

/// The first line of `text`, control characters stripped — the sanitize
/// every paste target applies before appending: the draft/query is always
/// rendered as a single row, so an embedded newline would only ever show as
/// a control glyph nobody typed.
pub(crate) fn sanitize_pasted_line(text: &str) -> String {
    text.lines()
        .next()
        .unwrap_or("")
        .chars()
        .filter(|c| !c.is_control())
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn erase_grapheme_pops_a_combining_mark_whole() {
        let mut text = "cafe\u{0301}".to_string();
        erase_grapheme(&mut text);
        assert_eq!(text, "caf");
    }

    #[test]
    fn erase_grapheme_on_empty_is_a_no_op() {
        let mut text = String::new();
        erase_grapheme(&mut text);
        assert_eq!(text, "");
    }

    #[test]
    fn sanitize_pasted_line_keeps_only_the_first_line_and_drops_control_chars() {
        assert_eq!(sanitize_pasted_line("ab\tc\ndef"), "abc");
    }

    #[test]
    fn sanitize_pasted_line_of_empty_input_is_empty() {
        assert_eq!(sanitize_pasted_line(""), "");
    }
}
