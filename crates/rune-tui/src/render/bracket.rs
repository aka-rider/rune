use rune_core::bracket::pair_at_caret;

use crate::document::Document;
use crate::theme::Theme;

use super::{Cell, paint_range};

pub(super) fn apply_bracket_match(rows: &mut [Vec<Cell>], doc: &Document, theme: &Theme) {
    if !doc.has_insertion_point() {
        return;
    }
    let content = doc.buffer.content();
    for cursor in doc.cursors.all() {
        let Some((open, close)) = pair_at_caret(content, cursor.position) else {
            continue;
        };
        for end in [open, close] {
            if end == cursor.position {
                continue;
            }
            paint_range(
                rows,
                end..end.saturating_add(1),
                theme.chrome.bracket_match_bg,
            );
        }
    }
}
