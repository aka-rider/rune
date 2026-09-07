use super::*;

use ratatui::style::Style;
use rune_core::buffer::Buffer;
use rune_core::coords::{BufferOffset, VisualCol};
use rune_core::cursor::{CursorSet, CursorSpec};

fn hits(content: &str, needle: &str, window: Range<usize>) -> Vec<Range<usize>> {
    hits_in_window(content, needle, window).collect()
}

#[test]
fn a_hit_starting_before_the_window_but_reaching_into_it_is_returned() {
    let content = "fox and fox";
    assert_eq!(hits(content, "fox", 7..11), vec![8..11]);
    assert_eq!(hits(content, "fox", 0..11), vec![0..3, 8..11]);
}

#[test]
fn a_hit_entirely_outside_the_window_is_not_returned() {
    assert_eq!(hits("fox and fox", "fox", 4..8), Vec::<Range<usize>>::new());
}

#[test]
fn a_needle_is_rejected_when_it_cannot_be_a_useful_hint() {
    assert_eq!(usable_needle(""), None);
    assert_eq!(usable_needle("   \t"), None);
    assert_eq!(usable_needle("fox\nfox"), None);
    assert_eq!(usable_needle("fox\rfox"), None);
    assert_eq!(usable_needle(&"x".repeat(MAX_NEEDLE_BYTES + 1)), None);
    assert_eq!(
        usable_needle(&"x".repeat(MAX_NEEDLE_BYTES)),
        Some("x".repeat(MAX_NEEDLE_BYTES).as_str())
    );
    assert_eq!(usable_needle("fox"), Some("fox"));
}

fn row_of(content: &str) -> Vec<Cell> {
    content
        .char_indices()
        .map(|(offset, ch)| Cell {
            text: ch.to_string().into(),
            width: 1,
            style: Style::default(),
            buf_offset: Some(offset as u32),
        })
        .collect()
}

fn doc_selecting(content: &str, start: usize, end: usize) -> Document {
    let mut doc = Document::new(Buffer::new(content));
    doc.cursors = CursorSet::new_from_specs(&[CursorSpec {
        position: BufferOffset(end),
        anchor: BufferOffset(start),
        desired_col: VisualCol(0),
    }]);
    doc
}

#[test]
fn painting_a_match_changes_only_the_cell_style() {
    let content = "fox fox";
    let theme = Theme::catppuccin_mocha(false);
    let before = row_of(content);
    let mut rows = vec![before.clone()];

    apply_selection_matches(&mut rows, &doc_selecting(content, 0, 3), &theme);

    let after = &rows[0];
    assert_eq!(after.len(), before.len());
    for (i, (a, b)) in after.iter().zip(before.iter()).enumerate() {
        assert_eq!(a.buf_offset, b.buf_offset, "cell {i} moved its buf_offset");
        assert_eq!(a.width, b.width, "cell {i} changed its width");
        assert_eq!(a.text, b.text, "cell {i} changed its text");
    }
    for (i, cell) in after.iter().enumerate() {
        let expected = if (4..7).contains(&i) {
            theme.chrome.selection_match_bg
        } else {
            Style::default()
        };
        assert_eq!(cell.style, expected, "cell {i} carries the wrong style");
    }
}
