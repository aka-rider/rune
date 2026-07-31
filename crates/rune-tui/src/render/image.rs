//! The per-row override entry point for an image document's cells (plan
//! WP4.S10/S11): a centered info card — file name, format, dimensions and
//! byte size, and a reason line — since this package renders NO pixels at
//! all (WP5 adds placeholder-cell transmission). `build_rows` calls
//! [`row_cells`] instead of `segment_cells` for any row whose `DisplayRow`
//! carries an `ImageRowRef` (the marker `rune-md`'s image producer,
//! `DocMachine::rebuild`'s `DocumentKind::Image` branch, sets on every row
//! it synthesizes).
//!
//! Every cell built here carries `buf_offset: -1` (plan gotcha: "`Style::
//! patch` is `or`, not overwrite" / "`place_caret`... no `buf_offset`
//! check" — a decorative cell must never be mistaken for one with real
//! buffer provenance by the cursor/selection/highlight overlays, all of
//! which already skip `buf_offset < 0`).

use ratatui::style::Style;

use rune_md::snapshot::ImageRowRef;

use crate::app::App;
use crate::document::Document;
use crate::graphics::ImageStatus;
use crate::render::Cell;

/// The fixed number of display rows the image producer reserves while no
/// pixel-based row count is known yet (plan WP4.S2, `Document::view`'s own
/// `set_image_dims` call) — enough to show every line of the info card
/// this module builds. WP5 overrides this with the real cell-based row
/// count once a decode's fit computation populates `ImageState::cells`.
pub const INFO_CARD_ROWS: usize = 4;

/// Builds one row of an image document's cells (plan WP4.S10/S11): the info
/// card's own `image_ref.row`'th line, centered within `width` columns and
/// padded to fill it. A row index past the card's own content (there are
/// more reserved rows than card lines, e.g. once WP5 reserves rows for the
/// image's real pixel height) renders blank.
pub fn row_cells(app: &App, doc: &Document, image_ref: ImageRowRef, width: u16) -> Vec<Cell> {
    let lines = info_card_lines(app, doc);
    let text = lines.get(image_ref.row).map(String::as_str).unwrap_or("");
    centered_cells(text, width as usize)
}

/// The info card's own lines, in display order: file name, format,
/// dimensions + byte size, and a reason line (plan WP4.S10).
fn info_card_lines(app: &App, doc: &Document) -> Vec<String> {
    let name = doc.file_name().to_string();
    let format = doc
        .file_path
        .as_deref()
        .and_then(|p| p.extension())
        .and_then(|e| e.to_str())
        .map(str::to_uppercase)
        .unwrap_or_else(|| "?".to_string());
    let dims = doc
        .image
        .as_ref()
        .and_then(|i| i.dims)
        .map(|(w, h)| format!("{w}x{h}"))
        .unwrap_or_else(|| "dimensions unknown".to_string());
    let size = doc
        .image
        .as_ref()
        .map(|i| human_size(i.bytes_len))
        .unwrap_or_default();
    vec![
        name,
        format,
        format!("{dims} \u{2014} {size}"),
        reason_line(app, doc),
    ]
}

/// The reason line: a non-Kitty terminal always shows the same message
/// regardless of decode status (plan WP4.S10) — there is nothing to show
/// there even if the decode itself would have succeeded. A Kitty-capable
/// terminal shows a status line, including `"decoding\u{2026}"` for the
/// `Pending` state every image document opens in (plan gotcha 9) — nothing
/// in WP4 ever advances it past `Pending`, since the decode `Cmd` itself is
/// WP5's.
fn reason_line(app: &App, doc: &Document) -> String {
    if !app.graphics.kitty {
        return "this terminal does not support inline images".to_string();
    }
    match doc.image.as_ref().map(|i| &i.status) {
        Some(ImageStatus::Pending) => "decoding\u{2026}".to_string(),
        Some(ImageStatus::Live) => String::new(),
        Some(ImageStatus::Failed(reason)) => format!("could not decode this image: {reason}"),
        None => "could not decode this image".to_string(),
    }
}

/// `bytes` as a human-readable size (`"512 B"`, `"1.2 KB"`, ...).
fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit = 0usize;
    while size >= 1024.0 && unit + 1 < UNITS.len() {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{size:.1} {}", UNITS.get(unit).copied().unwrap_or("TB"))
    }
}

/// `text` centered within `width` columns, padded to fill it — every cell
/// `buf_offset: -1`, width 1 (ASCII-only info-card text, so one cell per
/// byte). A `text` wider than `width` is left-aligned and simply clipped by
/// `blit`'s own area bound rather than truncated here.
fn centered_cells(text: &str, width: usize) -> Vec<Cell> {
    let len = text.chars().count();
    let pad = width.saturating_sub(len) / 2;
    let mut cells = Vec::with_capacity(width);
    for _ in 0..pad {
        cells.push(blank_cell());
    }
    for ch in text.chars() {
        cells.push(Cell {
            text: ch.to_string(),
            width: 1,
            style: Style::default(),
            buf_offset: -1,
        });
    }
    while cells.len() < width {
        cells.push(blank_cell());
    }
    cells
}

fn blank_cell() -> Cell {
    Cell {
        text: " ".to_string(),
        width: 1,
        style: Style::default(),
        buf_offset: -1,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use rune_core::buffer::Buffer;
    use rune_vfs::Mem;

    use super::*;
    use crate::graphics::ImageState;

    fn app_with_image_doc(kitty: bool, status: ImageStatus) -> App {
        let mut app = App::new(Buffer::new(""), None, Arc::new(Mem::new()), None);
        app.graphics.kitty = kitty;
        let id = app.active;
        let doc = app.doc_mut(id).expect("doc");
        doc.bind_path(PathBuf::from("/vault/x.png"));
        doc.read_only = true;
        doc.display_name = Some("x.png".to_string());
        doc.image = Some(ImageState {
            path: PathBuf::from("/vault/x.png"),
            bytes_len: 146,
            id: 1,
            dims: Some((64, 48)),
            cells: None,
            decoded: None,
            status,
            in_flight: None,
            pending: false,
        });
        app
    }

    #[test]
    fn info_card_lines_include_name_dims_and_kitty_reason() {
        let app = app_with_image_doc(true, ImageStatus::Pending);
        let doc = app.doc(app.active).expect("doc");
        let lines = info_card_lines(&app, doc);
        assert!(lines.iter().any(|l| l == "x.png"));
        assert!(lines.iter().any(|l| l.contains("64x48")));
        assert!(lines.iter().any(|l| l.contains("decoding")));
    }

    #[test]
    fn reason_line_ignores_status_when_kitty_is_unavailable() {
        let app = app_with_image_doc(false, ImageStatus::Live);
        let doc = app.doc(app.active).expect("doc");
        assert_eq!(
            reason_line(&app, doc),
            "this terminal does not support inline images"
        );
    }

    #[test]
    fn row_cells_center_the_requested_line() {
        let app = app_with_image_doc(true, ImageStatus::Pending);
        let doc = app.doc(app.active).expect("doc");
        let cells = row_cells(&app, doc, ImageRowRef { row: 0, width: 0 }, 20);
        assert_eq!(cells.len(), 20);
        let text: String = cells.iter().map(|c| c.text.as_str()).collect();
        assert!(text.contains("x.png"));
        for c in &cells {
            assert_eq!(c.buf_offset, -1);
        }
    }

    #[test]
    fn row_past_the_card_content_is_blank() {
        let app = app_with_image_doc(true, ImageStatus::Pending);
        let doc = app.doc(app.active).expect("doc");
        let cells = row_cells(&app, doc, ImageRowRef { row: 99, width: 0 }, 10);
        assert!(cells.iter().all(|c| c.text == " "));
    }

    #[test]
    fn human_size_formats_bytes_and_larger_units() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1536), "1.5 KB");
    }
}
