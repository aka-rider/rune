//! The per-row override entry point for an image row's cells (plan
//! WP4.S10/S11, extended WP9.S1 for inline embeds): a centered info card —
//! file name, format, dimensions and byte size, and a reason line — for a
//! whole `DocumentKind::Image` document with no pixels yet showable, and
//! Kitty placeholder cells for one that IS live; an inline embed row
//! (`ImageRowRef::target.is_some()`) instead shows placeholder cells with a
//! one-cell left margin when live, blank cells reserving its layout while
//! not yet live, or `None` (falling through to the row's own alt-text
//! span) when Kitty isn't available at all. `build_rows` calls
//! [`row_cells`] instead of `segment_cells` for any row whose `DisplayRow`
//! carries an `ImageRowRef` (the marker either the whole-document image
//! producer or, for an embed, `expand_images` sets on every row it
//! synthesizes).
//!
//! Every cell built here carries `buf_offset: -1` (plan gotcha: "`Style::
//! patch` is `or`, not overwrite" / "`place_caret`... no `buf_offset`
//! check" — a decorative cell must never be mistaken for one with real
//! buffer provenance by the cursor/selection/highlight overlays, all of
//! which already skip `buf_offset < 0`).

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

/// The fixed number of display rows the image producer reserves while no
/// pixel-based row count is known yet (plan WP4.S2, `Document::view`'s own
/// `set_image_document_dims` call) — enough to show every line of the info card
/// this module builds. WP5 overrides this with the real cell-based row
/// count once a decode's fit computation populates `ImageState::cells`.
pub const INFO_CARD_ROWS: usize = 4;

/// The single entry point `build_rows` calls for any row carrying an
/// `ImageRowRef` (plan WP4.S11/WP9.S1). Dispatches on
/// `image_ref.target`: `None` names a whole-document image row (exactly
/// one per document, `doc.image` answers everything — always `Some`, this
/// path never falls through to plain alt text since an image document has
/// no alt text of its own); `Some(target)` names an inline embed row,
/// looked up in `doc.embeds` by that key — may return `None` when Kitty
/// isn't available, letting `build_rows` fall through to the row's own
/// alt-text span instead (WP7's `Rendered` emit).
pub fn row_cells(
    app: &App,
    doc: &Document,
    image_ref: ImageRowRef,
    width: u16,
) -> Option<Vec<Cell>> {
    match &image_ref.target {
        None => Some(doc_image_row_cells(app, doc, &image_ref, width)),
        Some(target) => embed_row_cells(app, doc, target, &image_ref, width),
    }
}

/// A whole `DocumentKind::Image` document's row (plan WP4.S10/S11, extended
/// WP5.S4): a `Live` image on a Kitty-capable terminal renders real
/// placeholder cells (`live_row_cells`) carrying the smuggled 24-bit id;
/// every other case (no Kitty, still `Pending`/`Failed`) falls back to the
/// info card's own `image_ref.row`'th line, centered within `width`
/// columns and padded to fill it. A row index past the card's own content
/// (there are more reserved rows than card lines) renders blank.
fn doc_image_row_cells(
    app: &App,
    doc: &Document,
    image_ref: &ImageRowRef,
    width: u16,
) -> Vec<Cell> {
    if app.graphics.kitty
        && let Some(image) = &doc.image
        && image.status == ImageStatus::Live
    {
        return live_row_cells(image.id, image_ref.row, image_ref.width, width as usize, 0);
    }
    let lines = info_card_lines(app, doc);
    let text = lines.get(image_ref.row).map(String::as_str).unwrap_or("");
    centered_cells(text, width as usize)
}

/// One inline embed's row (plan WP9.S1): Kitty + `Live` -> placeholder
/// cells preceded by one blank left-margin cell (`margin: 1` below) so an
/// embed's pixels never sit flush against the preceding column — the whole
/// document producer's own rows (no margin) never need this, since a whole
/// image document has nothing else sharing its rows. Kitty + not yet live
/// (untracked, still `Pending`, or `Failed`) -> blank cells of the row's
/// full reserved width, holding the layout without pointing the terminal
/// at pixels that were never transmitted. No Kitty -> `None`, so
/// `build_rows` falls through to the row's own alt-text span instead.
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
    match doc.embeds.images.get(target) {
        Some(embed) if embed.status == ImageStatus::Live => Some(live_row_cells(
            embed.id,
            image_ref.row,
            image_ref.width,
            width,
            1,
        )),
        _ => Some(vec![blank_cell(); width]),
    }
}

/// A `Live` image row's real cells (plan WP5.S4, extended WP9.S1's
/// `margin`): `margin` blank cells (0 for a whole document, 1 for an
/// embed), then up to `image_cols` (the producer's own reserved column
/// count for this row, capped by `width`) placeholder cells, each
/// `PLACEHOLDER` + a row diacritic + a column diacritic — the Kitty Unicode
/// placeholder protocol's own encoding of WHICH cell of the image this is
/// — padded with blank cells out to `width`. Every cell is `width: 1`
/// (`blit` resets the continuation columns of a wide cell, which would
/// wipe the smuggled id) and `buf_offset: -1` (protects it from the
/// syntax/selection/caret overlays, all of which skip a negative offset).
/// `style.fg` carries the allocated Kitty image id as a 24-bit RGB colour
/// — the ONLY way to smuggle an arbitrary colour past `segment_cells`'
/// theme-lookup-only span styling (see this module's own doc comment).
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
        let mut text = String::with_capacity(rune_image::PLACEHOLDER.len_utf8() * 3);
        text.push(rune_image::PLACEHOLDER);
        text.push(rune_image::diacritic(row));
        text.push(rune_image::diacritic(col));
        cells.push(Cell {
            text,
            width: 1,
            style: Style::default().fg(fg),
            buf_offset: -1,
        });
    }
    while cells.len() < width {
        cells.push(blank_cell());
    }
    cells
}

/// The allocated Kitty image id, reinterpreted as a 24-bit RGB colour
/// (plan WP5.S4) — `rune_image::alloc_id` already masks its result to
/// `0x00FF_FFFF`, so the top byte is always zero and every id round-trips
/// through this split without loss.
fn id_to_rgb(id: u32) -> Color {
    Color::Rgb(
        ((id >> 16) & 0xFF) as u8,
        ((id >> 8) & 0xFF) as u8,
        (id & 0xFF) as u8,
    )
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
/// regardless of decode status — there is nothing to show there even if the
/// decode itself would have succeeded.
///
/// On a Kitty-capable terminal, `Pending` is deliberately split by whether a
/// decode is actually in flight. `Pending` alone does NOT mean one is
/// running — it is equally the state of a document whose decode was never
/// scheduled, or whose reply was lost. Collapsing both into
/// `"decoding\u{2026}"` is what made a wedged decode indistinguishable from a
/// slow one: the card read the same forever, with nothing to suggest the
/// reload key would help.
fn reason_line(app: &App, doc: &Document) -> String {
    if !app.graphics.kitty {
        return "this terminal does not support inline images".to_string();
    }
    let Some(image) = doc.image.as_ref() else {
        return "could not decode this image".to_string();
    };
    match &image.status {
        ImageStatus::Pending if image.in_flight.is_some() => "decoding\u{2026}".to_string(),
        ImageStatus::Pending => "not decoded — press the reload key to retry".to_string(),
        ImageStatus::Live => String::new(),
        ImageStatus::Failed(reason) => format!("could not decode this image: {reason}"),
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
/// `buf_offset: -1`. `text` names a user file (the card's file-name line, or
/// an inline embed's link target), so it is NOT ASCII-only and is not even
/// guaranteed printable: it is routed through `push_grapheme_cells` — the
/// SAME chokepoint `segment_cells` uses for real buffer content — one
/// GRAPHEME CLUSTER at a time, never one `Cell` per `char`. A bare `char`
/// walk would split a ZWJ emoji or a base+combining-mark cluster apart, and
/// a hardcoded `width: 1` would mislabel a CJK cluster's true 2-cell width,
/// both of which `blit`'s own wide-cell handling (and ratatui's buffer
/// diffing beneath it) depend on being correct; reusing the chokepoint also
/// inherits its control-character handling for free — a raw control byte in
/// a file name or link target (a tab, a bare `\r`) is replaced with its
/// safe placeholder glyph rather than reaching ratatui's buffer, which
/// panics on an unfiltered ASCII control byte.
///
/// The content's width used to compute the leading/trailing pad is NEVER a
/// second, independently-computed number (a prior version measured with
/// `crate::width::display_width` while building with `push_grapheme_cells`
/// — the two disagree on a TAB, whose expansion width depends on its
/// starting column, so the declared row could silently overrun `width`).
/// Instead this builds the content cells twice through the ONE chokepoint:
/// a first pass at column 0 only to learn how many columns the content
/// needs (`probe_width`, below) so `pad` can be chosen; a second pass that
/// actually becomes the returned cells, started at column `pad` — the
/// column the content will really sit at once the leading blanks precede
/// it — so a tab inside `text` expands to the same stop the finished row
/// renders it at, not the stop it would hit at column 0. `pad` is then
/// clamped so the real pass's own width can never push the total past
/// `width`, even in the (bounded, since a tab expands to at most 4 cells)
/// case where restarting the tab math at column `pad` instead of `0` makes
/// the content wider than the probe predicted — the row's declared total
/// width is always exactly `width`, never more.
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

/// `text` as `Cell`s through the `push_grapheme_cells` chokepoint, with the
/// running visual column starting at `start_col` — the column the caller
/// intends this content to actually occupy, so a tab's expansion (which
/// depends on the column it starts at) matches where it will really render.
fn grapheme_cells(text: &str, start_col: usize) -> Vec<Cell> {
    let mut cells = Vec::new();
    let mut visual_col = start_col;
    for grapheme in text.graphemes(true) {
        push_grapheme_cells(&mut cells, &mut visual_col, grapheme, -1, Style::default());
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

// Kept in a sibling file: this module's own row-building code
// stays under the 500-line budget on its own merits.
#[cfg(test)]
#[path = "image_tests.rs"]
mod tests;
