//! Wrapped layout (WP4.S4): a table row that no longer fits Grid but can
//! still be shrunk into a readable, proportionally narrower box, with each
//! cell's own text word-wrapped across as many visual rows as the widest
//! cell in that row needs. Split out of `layout.rs` on its own —
//! Wrapped's own row builder shares `layout`'s
//! `FlatChar`/`group_runs` plumbing but is otherwise a distinct concern
//! from Grid geometry.

use unicode_segmentation::UnicodeSegmentation;

use rune_syntax::ScopeId;
use rune_syntax::wrap::grapheme_width;

use super::CellSrc;
use super::layout::{FlatChar, display_width, group_runs};
use crate::emit::style;

/// `true` for a token this crate never breaks mid-word when wrapping a
/// cell — a bare policy check, unrelated to a word's own display width.
fn is_url(word: &str) -> bool {
    word.starts_with("http://") || word.starts_with("https://")
}

/// Word-wraps one cell's rendered `text` to `max_width` display cells:
/// trims, splits on whitespace, greedy-packs words by accumulated
/// `grapheme_width`. A `http://`/`https://`-prefixed word is atomic and
/// never broken; any OTHER over-long word is hard-broken, at accumulated
/// DISPLAY width (Assumption A2: never by rune COUNT, which would overflow
/// a CJK column by up to 2x — a rune there is one array slot but two
/// display cells). Wrapped rows are always left-aligned — this returns
/// plain lines; the caller pads and aligns.
pub fn wrap_cell(text: &str, max_width: usize) -> Vec<String> {
    let trimmed = text.trim();
    if max_width == 0 || display_width(trimmed) <= max_width {
        return vec![trimmed.to_string()];
    }

    let words: Vec<&str> = trimmed.split_whitespace().collect();
    if words.is_empty() {
        return vec![String::new()];
    }

    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut current_width = 0usize;

    for word in words {
        let word_width = display_width(word);
        if current_width == 0 {
            place_first_word(
                word,
                word_width,
                max_width,
                &mut lines,
                &mut current,
                &mut current_width,
            );
            continue;
        }
        if current_width + 1 + word_width <= max_width {
            current.push(' ');
            current.push_str(word);
            current_width += 1 + word_width;
        } else {
            lines.push(std::mem::take(&mut current));
            current_width = 0;
            place_first_word(
                word,
                word_width,
                max_width,
                &mut lines,
                &mut current,
                &mut current_width,
            );
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// Places `word` as the first word of a (possibly new) wrapped line: an
/// over-long non-URL word is hard-broken (`hard_break_word`); anything else
/// — including an over-long URL, which this never breaks — is placed
/// whole, overflow and all: the first word on a line is always placed,
/// even past `max_width`.
fn place_first_word(
    word: &str,
    word_width: usize,
    max_width: usize,
    lines: &mut Vec<String>,
    current: &mut String,
    current_width: &mut usize,
) {
    if word_width > max_width && !is_url(word) {
        let (mut completed, rem, rem_w) = hard_break_word(word, max_width);
        lines.append(&mut completed);
        *current = rem;
        *current_width = rem_w;
    } else {
        *current = word.to_string();
        *current_width = word_width;
    }
}

/// Hard-breaks `word` into chunks whose accumulated `grapheme_width` never
/// exceeds `max_width` (Assumption A2) — returns every FULL chunk as a
/// completed line, plus the final (possibly under-full) chunk and its width
/// for the caller to keep accumulating onto. A single grapheme cluster
/// wider than `max_width` still becomes its own chunk (an unsplittable
/// cluster is placed whole rather than silently truncated — no
/// caller-visible content is ever dropped).
fn hard_break_word(word: &str, max_width: usize) -> (Vec<String>, String, usize) {
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_width = 0usize;
    for g in word.graphemes(true) {
        let gw = grapheme_width(g);
        if !current.is_empty() && current_width + gw > max_width {
            lines.push(std::mem::take(&mut current));
            current_width = 0;
        }
        current.push_str(g);
        current_width += gw;
    }
    (lines, current, current_width)
}

/// One Wrapped-layout visual row: `│` opens/closes every column exactly
/// like `layout::grid_row`, always left-aligned, but EVERY char — content,
/// padding, and border alike — is decorative (`buf = None`). Never keeps a
/// real per-char buffer offset for this layout at all, unlike Grid — once
/// word-wrap has reshuffled a cell's content across several visual rows,
/// "this char came from that one buffer byte" is no longer a claim this
/// renders makes.
pub fn wrapped_row(
    widths: &[usize],
    wrapped_cells: &[Vec<String>],
    visual_row: usize,
    role_scope: ScopeId,
) -> Vec<(String, Vec<CellSrc>, ScopeId)> {
    let border = style::table_border_scope();
    let mut flat: Vec<FlatChar> = Vec::new();
    flat.push(FlatChar {
        ch: '│',
        buf: None,
        scope: border,
    });
    for (i, &w) in widths.iter().enumerate() {
        let cell_text = wrapped_cells
            .get(i)
            .and_then(|lines| lines.get(visual_row))
            .map(String::as_str)
            .unwrap_or("");
        flat.push(FlatChar {
            ch: ' ',
            buf: None,
            scope: role_scope,
        });
        let content_w = display_width(cell_text);
        for ch in cell_text.chars() {
            flat.push(FlatChar {
                ch,
                buf: None,
                scope: role_scope,
            });
        }
        for _ in 0..w.saturating_sub(content_w) {
            flat.push(FlatChar {
                ch: ' ',
                buf: None,
                scope: role_scope,
            });
        }
        flat.push(FlatChar {
            ch: ' ',
            buf: None,
            scope: role_scope,
        });
        flat.push(FlatChar {
            ch: '│',
            buf: None,
            scope: border,
        });
    }
    group_runs(&flat)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn wrap_cell_fits_short_text_on_one_line() {
        assert_eq!(wrap_cell("hi", 10), vec!["hi".to_string()]);
    }

    #[test]
    fn wrap_cell_greedily_packs_words_by_display_width() {
        let lines = wrap_cell("one two three four", 9);
        assert_eq!(lines, vec!["one two", "three", "four"]);
    }

    #[test]
    fn wrap_cell_never_breaks_a_url() {
        let url = "https://example.com/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let lines = wrap_cell(url, 10);
        assert_eq!(lines, vec![url.to_string()]);
    }

    #[test]
    fn wrap_cell_hard_breaks_an_over_long_non_url_word_by_display_width() {
        let lines = wrap_cell("世界世界世界", 4);
        // Each CJK char is display-width 2, so a width-4 chunk holds exactly
        // 2 chars — never 4 (Assumption A2: not rune-count chunking, which
        // would put 4 runes = 8 display cells into one "4-wide" chunk).
        assert_eq!(lines, vec!["世界", "世界", "世界"]);
    }

    #[test]
    fn wrapped_row_pads_content_left_aligned_and_marks_every_char_decorative() {
        let widths = vec![5];
        let wrapped_cells = vec![vec!["ab".to_string()]];
        let runs = wrapped_row(&widths, &wrapped_cells, 0, ScopeId(9));
        let text: String = runs.iter().map(|(t, _, _)| t.as_str()).collect();
        // `Σw + 3n + 1` with n=1, w=5: 9 chars total — `│`, one left-pad
        // space, "ab", enough fill + the right-pad space to reach width 5
        // worth of content, `│`.
        assert_eq!(text.chars().count(), 5 + 3 + 1);
        assert!(text.starts_with("│ ab"));
        assert!(text.ends_with('│'));
        for (_, src, _) in &runs {
            assert!(src.iter().all(|c| c.buf.is_none()));
        }
    }
}
