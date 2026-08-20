use rune_core::bracket::bracket_pair;

use crate::document::Document;
use crate::theme::Theme;

use super::{Cell, paint_range};

pub(super) fn apply_bracket_match(rows: &mut [Vec<Cell>], doc: &Document, theme: &Theme) {
    if !doc.has_insertion_point() {
        return;
    }
    let content = doc.buffer.content();
    for cursor in doc.cursors.all() {
        let Some((open, close)) = bracket_pair(content, cursor.position) else {
            continue;
        };
        let far = if open == cursor.position { close } else { open };
        paint_range(
            rows,
            far..far.saturating_add(1),
            theme.chrome.bracket_match_bg,
        );
    }
}
