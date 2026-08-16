use rune_core::buffer::Buffer;

use crate::app::App;
use crate::document::{Document, DocumentId, ReadOnly};

#[derive(Debug, PartialEq, Eq)]
pub enum DiffInstallError {
    InvalidUtf8,
}

pub struct DiffView {
    pub left: Document,
    pub left_name: String,
    pub right: DocumentId,
    pub hunk_cur: usize,
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
    app.diff = Some(DiffView {
        left,
        left_name,
        right: app.active,
        hunk_cur: 0,
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
    let Some(right_scroll) = app.doc(right_id).map(|d| d.viewport.scroll_row) else {
        return;
    };
    let Some(diff) = app.diff.as_mut() else {
        return;
    };
    let view = diff.left.sync();
    diff.left.view = Some(view);
    diff.left.viewport.scroll_row = right_scroll;
    let total_rows = diff
        .left
        .view
        .as_ref()
        .map_or(0, |v| v.display.total_rows());
    diff.left.viewport.clamp_to_document(total_rows);
}
