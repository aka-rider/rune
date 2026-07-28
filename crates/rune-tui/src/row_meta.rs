//! `RowMeta` — per-display-row table membership metadata, sampled beside
//! `render::build_rows` (plan WP5.S1) purely as a signal for the session
//! fuzzer's `TABLE-ROW-WIDTH`/`TABLE-SYNTHETIC-DECORATIVE` invariants.
//! Kept out of `render.rs` (already over the §1.6 line budget) since no
//! cell-building code ever reads it.

use rune_md::element::doc::ViewSnapshots;

use crate::app::App;

/// One visible display row's table affiliation, index-aligned with
/// `render::build_rows`'s own output (`row_meta` below windows
/// `view.display.rows()` through the SAME `viewport.scroll_row`/`height`
/// `build_rows` uses), so `Snapshot.cells[i]` and `Snapshot.row_meta[i]`
/// always describe the same row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowMeta {
    /// Mirrors `DisplayRow::synthetic` (`rune_md::snapshot`) — a
    /// synthesised border row with no source line at all.
    pub synthetic: bool,
    /// `Some(n)` for every row — content or synthetic border — that
    /// belongs to a table, where `n` increments once per contiguous run of
    /// table-affiliated display rows in THIS window; `None` for a row with
    /// no table affiliation at all.
    pub table_group: Option<usize>,
}

/// Builds one `RowMeta` per row `render::build_rows(view, app)` returns,
/// in the same order. A row is table-affiliated if it is synthetic (a
/// border row only ever exists adjacent to a table —
/// `DisplaySnapshot::expand_tables`'s docs) or if its own wrap segment
/// carries `TableSegInfo` (`WrapSegment::table`); a run of such rows with
/// no non-table row between them shares one `table_group` id.
pub fn row_meta(view: &ViewSnapshots, app: &App) -> Vec<RowMeta> {
    let doc = app.active_doc();
    let viewport = &doc.viewport;
    let height = viewport.height as usize;
    let segments = view.wrap.segments();

    let mut out = Vec::new();
    let mut current_group: Option<usize> = None;
    let mut next_id = 0usize;

    for row in view
        .display
        .rows()
        .iter()
        .skip(viewport.scroll_row)
        .take(height)
    {
        let is_table = row.synthetic
            || segments
                .get(row.wrap_row)
                .is_some_and(|seg| seg.table.is_some());

        let group = if is_table {
            if current_group.is_none() {
                current_group = Some(next_id);
                next_id += 1;
            }
            current_group
        } else {
            current_group = None;
            None
        };

        out.push(RowMeta {
            synthetic: row.synthetic,
            table_group: group,
        });
    }

    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use std::sync::Arc;

    use rune_core::buffer::Buffer;
    use rune_core::cursor::CursorSet;
    use rune_vfs::{Mem, Vfs};

    use super::*;

    /// Builds a focused `App` over `content` with the cursor pinned at
    /// `content.len()` — OUTSIDE every table's own line range for every
    /// fixture this module's tests use, so reveal-on-cursor (plan
    /// architectural decision 5: a table with the cursor on one of its own
    /// lines renders its raw source, not borders) never turns the very
    /// thing these tests assert on back into plain pipe-and-dash text.
    fn app_for(content: &str) -> App {
        let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Mem::new());
        let mut app = App::new(Buffer::new(content), None, vfs, None);
        app.active_doc_mut().focused = true;
        app.active_doc_mut().cursors = CursorSet::new_from_positions(&[content.len()]);
        app.frame_width = 80;
        app.frame_height = 24;
        app.relayout();
        app.sync_view();
        app
    }

    #[test]
    fn table_rows_share_one_group_and_borders_are_synthetic() {
        // A blank line then a trailing "tail" line, OUTSIDE the table's
        // own line range — the blank line is what actually ends the GFM
        // table (a bare non-pipe line directly after the last row is
        // still absorbed as a ragged table row, not prose); `app_for`
        // pins the cursor at `content.len()`, landing on "tail", so the
        // table itself stays Rendered (reveal-on-cursor would otherwise
        // flip it back to raw pipe-and-dash source).
        let content = "| Name | Age |\n| --- | --- |\n| Alice | 30 |\n| Bob | 25 |\n\ntail";
        let app = app_for(content);
        let view = app.active_doc().view.as_ref().expect("view must be built");
        let meta = row_meta(view, &app);

        // Border, header, separator, body, body, border, then the blank
        // line and "tail" — one contiguous group covering exactly the
        // table, no gaps, nothing bleeding into what follows it.
        let (table_rows, rest) = meta.split_at(meta.len() - 2);
        assert!(table_rows.iter().all(|m| m.table_group == Some(0)));
        assert!(rest.iter().all(|m| m.table_group.is_none() && !m.synthetic));
        assert!(table_rows.first().is_some_and(|m| m.synthetic));
        assert!(table_rows.last().is_some_and(|m| m.synthetic));
        assert_eq!(table_rows.iter().filter(|m| !m.synthetic).count(), 4);
    }

    #[test]
    fn prose_rows_carry_no_table_group() {
        let content = "just a line of prose\nand another\n";
        let app = app_for(content);
        let view = app.active_doc().view.as_ref().expect("view must be built");
        let meta = row_meta(view, &app);

        assert!(meta.iter().all(|m| m.table_group.is_none() && !m.synthetic));
    }

    #[test]
    fn two_tables_separated_by_prose_get_distinct_groups() {
        let content = "| a | b |\n| - | - |\n| 1 | 2 |\n\ngap\n\n| c | d |\n| - | - |\n| 3 | 4 |\n";
        let app = app_for(content);
        let view = app.active_doc().view.as_ref().expect("view must be built");
        let meta = row_meta(view, &app);

        let groups: Vec<Option<usize>> = meta.iter().map(|m| m.table_group).collect();
        assert!(groups.contains(&Some(0)));
        assert!(groups.contains(&Some(1)));
        assert!(groups.contains(&None));
    }
}
