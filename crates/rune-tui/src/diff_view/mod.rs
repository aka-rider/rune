use rune_core::buffer::Buffer;
use rune_core::coords::DisplayRow;
use rune_merge::AlignmentMap;

use crate::app::App;
use crate::document::{Document, DocumentId, ReadOnly};

pub mod rows;

#[derive(Debug, PartialEq, Eq)]
pub enum DiffInstallError {
    InvalidUtf8,
}

pub struct DiffView {
    pub left: Document,
    pub left_name: String,
    pub right: DocumentId,
    pub hunk_cur: usize,
    pub alignment: AlignmentMap,
    right_version: u64,
}

pub fn install(
    app: &mut App,
    left_bytes: Vec<u8>,
    left_name: String,
) -> Result<(), DiffInstallError> {
    let text = String::from_utf8(left_bytes).map_err(|_| DiffInstallError::InvalidUtf8)?;
    let mut left = Document::new(Buffer::new(text));
    left.read_only = ReadOnly::Always;
    left.focused = false;
    left.display_name = Some(left_name.clone());
    let right_content = app.active_doc().buffer.content().to_string();
    let alignment = rune_merge::align(left.buffer.content(), &right_content);
    let right_version = app.active_doc().buffer.version();
    app.diff = Some(DiffView {
        left,
        left_name,
        right: app.active,
        hunk_cur: 0,
        alignment,
        right_version,
    });
    Ok(())
}

pub fn sync(app: &mut App) {
    let Some(right_id) = app.diff.as_ref().map(|d| d.right) else {
        return;
    };
    if app.active != right_id {
        return;
    }
    let Some(right_doc) = app.doc(right_id) else {
        return;
    };
    let right_scroll = right_doc.viewport.scroll_row;
    let right_version = right_doc.buffer.version();
    let right_content = right_doc.buffer.content().to_string();
    let right_heights = right_doc
        .view
        .as_ref()
        .map(|v| rows::line_heights(&v.wrap))
        .unwrap_or_default();

    let Some(diff) = app.diff.as_mut() else {
        return;
    };
    let view = diff.left.sync();
    diff.left.view = Some(view);

    if diff.right_version != right_version {
        diff.alignment = rune_merge::align(diff.left.buffer.content(), &right_content);
        diff.right_version = right_version;
    }

    let left_heights = diff
        .left
        .view
        .as_ref()
        .map(|v| rows::line_heights(&v.wrap))
        .unwrap_or_default();
    let layout = rows::layout_rows(&diff.alignment, &left_heights, &right_heights);
    diff.left.viewport.scroll_row =
        DisplayRow(rows::left_row_for_right_row(&layout, right_scroll.0));

    let total_rows = diff
        .left
        .view
        .as_ref()
        .map_or(0, |v| v.display.total_rows());
    diff.left.viewport.clamp_to_document(total_rows);
}
