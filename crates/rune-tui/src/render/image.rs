use ratatui::style::{Color, Style};
use unicode_segmentation::UnicodeSegmentation;

use rune_md::snapshot::ImageRowRef;

use crate::app::App;
use crate::document::Document;
use crate::graphics::ImageStatus;
use crate::render::Cell;
use crate::render::cell::push_grapheme_cells;
#[cfg(test)]
use crate::width::display_width;

pub const INFO_CARD_ROWS: usize = 4;

const EMBED_LEFT_MARGIN: usize = 1;

pub fn row_cells(
    app: &App,
    doc: &Document,
    image_ref: &ImageRowRef,
    width: u16,
) -> Option<Vec<Cell>> {
    image_ref.target.as_ref().map_or_else(
        || Some(doc_image_row_cells(app, doc, image_ref, width)),
        |target| embed_row_cells(app, doc, target, image_ref, width),
    )
}

fn doc_image_row_cells(
    app: &App,
    doc: &Document,
    image_ref: &ImageRowRef,
    width: u16,
) -> Vec<Cell> {
    if app.graphics.kitty
        && let Some(image) = doc.image()
        && matches!(image.status, ImageStatus::Live { .. })
    {
        return live_row_cells(
            image.id.get(),
            image_ref.row,
            image_ref.width,
            width as usize,
            0,
        );
    }
    let lines = info_card_lines(app, doc);
    let text = lines.get(image_ref.row).map_or("", String::as_str);
    centered_cells(text, width as usize)
}

fn embed_row_cells(
    app: &App,
    doc: &Document,
    target: &str,
    image_ref: &ImageRowRef,
    width: u16,
) -> Option<Vec<Cell>> {
    if !app.graphics.kitty {
        return None;
    }
    let width = width as usize;
    match doc.embeds().and_then(|embeds| embeds.images.get(target)) {
        Some(embed) if matches!(embed.status, ImageStatus::Live { .. }) => Some(live_row_cells(
            embed.id.get(),
            image_ref.row,
            image_ref.width,
            width,
            EMBED_LEFT_MARGIN,
        )),
        _ => Some(vec![blank_cell(); width]),
    }
}

// The Kitty Unicode placeholder protocol: `PLACEHOLDER` plus a row
// diacritic and a column diacritic names which cell of the image this is.
// Every cell is `width: 1` (`blit` resets a wide cell's continuation
// columns, which would erase the smuggled id) and `buf_offset: None`
// (keeps it out of the syntax/selection/caret overlays). `style.fg`
// carries the allocated Kitty image id as a 24-bit RGB colour.
fn live_row_cells(
    id: u32,
    row: usize,
    image_cols: usize,
    width: usize,
    margin: usize,
) -> Vec<Cell> {
    let fg = id_to_rgb(id);
    let mut cells = Vec::with_capacity(width);
    for _ in 0..margin.min(width) {
        cells.push(blank_cell());
    }
    let cols = image_cols.min(width.saturating_sub(cells.len()));
    for col in 0..cols {
        let mut text =
            compact_str::CompactString::with_capacity(rune_image::PLACEHOLDER.len_utf8() * 3);
        text.push(rune_image::PLACEHOLDER);
        text.push(rune_image::diacritic(row));
        text.push(rune_image::diacritic(col));
        cells.push(Cell {
            text,
            width: 1,
            style: Style::default().fg(fg),
            buf_offset: None,
        });
    }
    while cells.len() < width {
        cells.push(blank_cell());
    }
    cells
}

// `rune_image::alloc_id` masks its result to 0x00FF_FFFF, so the top byte
// is always zero and every id round-trips through this split losslessly.
fn id_to_rgb(id: u32) -> Color {
    Color::Rgb(
        ((id >> 16) & 0xFF) as u8,
        ((id >> 8) & 0xFF) as u8,
        (id & 0xFF) as u8,
    )
}

fn info_card_lines(app: &App, doc: &Document) -> Vec<String> {
    let name = doc.file_name().to_string();
    let format = doc
        .file_path
        .as_deref()
        .and_then(|p| p.extension())
        .and_then(|e| e.to_str())
        .map_or_else(|| "?".to_string(), str::to_uppercase);
    let dims = doc.image().and_then(|i| i.dims).map_or_else(
        || "dimensions unknown".to_string(),
        |d| format!("{}x{}", d.w, d.h),
    );
    let size = doc
        .image()
        .map(|i| human_size(i.bytes_len))
        .unwrap_or_default();
    vec![
        name,
        format,
        format!("{dims} \u{2014} {size}"),
        reason_line(app, doc),
    ]
}

fn reason_line(app: &App, doc: &Document) -> String {
    if !app.graphics.kitty {
        return "this terminal does not support inline images".to_string();
    }
    let Some(image) = doc.image() else {
        return "could not decode this image".to_string();
    };
    match &image.status {
        ImageStatus::Pending if image.in_flight.is_some() => "decoding\u{2026}".to_string(),
        ImageStatus::Pending => "not decoded — press the reload key to retry".to_string(),
        ImageStatus::Live { .. } => String::new(),
        ImageStatus::Failed(reason) => format!("could not decode this image: {reason}"),
    }
}

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

fn centered_cells(text: &str, width: usize) -> Vec<Cell> {
    let probe_width: usize = grapheme_cells(text, 0)
        .iter()
        .map(|c| c.width as usize)
        .sum();
    let pad = width.saturating_sub(probe_width) / 2;

    let content = grapheme_cells(text, pad);
    let content_width: usize = content.iter().map(|c| c.width as usize).sum();
    let pad = pad.min(width.saturating_sub(content_width));

    let mut cells = Vec::with_capacity(width);
    for _ in 0..pad {
        cells.push(blank_cell());
    }
    cells.extend(content);
    let used = pad + content_width;
    for _ in used..width {
        cells.push(blank_cell());
    }
    cells
}

fn grapheme_cells(text: &str, start_col: usize) -> Vec<Cell> {
    let mut cells = Vec::new();
    let mut visual_col = start_col;
    for grapheme in text.graphemes(true) {
        push_grapheme_cells(
            &mut cells,
            &mut visual_col,
            grapheme,
            None,
            Style::default(),
        );
    }
    cells
}

fn blank_cell() -> Cell {
    Cell {
        text: " ".into(),
        width: 1,
        style: Style::default(),
        buf_offset: None,
    }
}

#[cfg(test)]
#[path = "image_tests.rs"]
mod tests;
