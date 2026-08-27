use rune_md::element::doc::ViewSnapshots;

use crate::app::App;

// Index-aligned with `render::build_rows`'s own output, so `cells[i]` and
// `row_meta[i]` always describe the same row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowMeta {
    // A synthesised border row with no source line of its own.
    pub synthetic: bool,
    // `Some(n)` for every row belonging to a table, `n` incrementing once
    // per contiguous run of table-affiliated rows in this window; `None`
    // otherwise.
    pub table_group: Option<usize>,
    // Grid and Wrapped tables draw a box and pad every row to one shared
    // width; the Pivoted key-value layout does not, and its rows are
    // deliberately ragged, so an equal-width expectation only holds here.
    pub boxed: bool,
}

// A row is table-affiliated if it is synthetic (a border row only ever
// exists adjacent to a table) or its own wrap segment carries table info;
// a run of such rows with no non-table row between them shares one
// `table_group` id.
pub fn row_meta(view: &ViewSnapshots, app: &App) -> Vec<RowMeta> {
    let doc = app.active_doc();
    let viewport = &doc.viewport;
    let segments = view.wrap.segments();

    let mut out = Vec::new();
    let mut current_group: Option<usize> = None;
    let mut next_id = 0usize;

    for row in crate::viewport::visible_rows(view.display.rows(), viewport) {
        let is_table = row.synthetic
            || segments
                .get(row.wrap_row)
                .is_some_and(|seg| seg.table.is_some());

        let boxed = segments
            .get(row.wrap_row)
            .and_then(|seg| seg.table.as_ref())
            .is_some_and(|t| t.boxed)
            // A synthetic border row only ever exists around a boxed table,
            // so it is boxed by construction even where the lookup above
            // can't see the flag.
            || row.synthetic;

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
            boxed,
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

    // Pins the cursor at `content.len()`, outside every table's own line
    // range in every fixture below — reveal-on-cursor renders a table's raw
    // source instead of borders when the cursor sits on one of its lines,
    // which would undo the very thing these tests assert on.
    fn app_for(content: &str) -> App {
        let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Mem::new());
        let mut app = App::new(Buffer::new(content), None, vfs, None);
        app.active_doc_mut().focused = true;
        app.active_doc_mut().cursors = CursorSet::new_from_positions(&[content.len()]);
        app.frame = Some(crate::app::FrameSize::new(80, 24));
        app.relayout();
        app.sync_view();
        app
    }

    #[test]
    fn table_rows_share_one_group_and_borders_are_synthetic() {
        // A bare non-pipe line directly after a GFM table's last row is
        // still absorbed as a ragged table row, not prose, so the fixture
        // needs a blank line before "tail" to actually end the table.
        let content = "| Name | Age |\n| --- | --- |\n| Alice | 30 |\n| Bob | 25 |\n\ntail";
        let app = app_for(content);
        let view = app.active_doc().view.as_ref().expect("view must be built");
        let meta = row_meta(view, &app);

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
