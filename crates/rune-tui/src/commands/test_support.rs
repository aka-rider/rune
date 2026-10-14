#![cfg(test)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use rune_core::coords::{BufferOffset, VisualCol};
use rune_core::cursor::{Cursor, CursorSet, CursorSpec};

use crate::app::App;
use crate::document::DocumentId;

pub(crate) fn selecting(app: &mut App, id: DocumentId, anchor: usize, position: usize) {
    let primary = app.doc(id).unwrap().cursors.primary();
    app.doc_mut(id).unwrap().cursors = CursorSet::new_from(&[Cursor {
        anchor: BufferOffset(anchor),
        position: BufferOffset(position),
        ..primary
    }]);
}

pub(crate) fn cursors(app: &mut App, id: DocumentId, sel: &[(usize, usize)]) {
    let specs: Vec<CursorSpec> = sel
        .iter()
        .map(|&(anchor, position)| CursorSpec {
            anchor: BufferOffset(anchor),
            position: BufferOffset(position),
            desired_col: VisualCol(0),
        })
        .collect();
    app.doc_mut(id).unwrap().cursors = CursorSet::new_from_specs(&specs);
}
