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
use rune_syntax::wrap::grapheme_width;
use unicode_segmentation::UnicodeSegmentation;

use rune_md::snapshot::ImageRowRef;

use crate::app::App;
use crate::document::Document;
use crate::graphics::ImageStatus;
use crate::render::Cell;
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
/// an inline embed's link target), so it is NOT ASCII-only: it is segmented
/// one `Cell` per GRAPHEME CLUSTER, each at its real `grapheme_width` (the
/// same chokepoint `push_grapheme_cells` uses), never one `Cell` per `char`
/// — a bare `char` walk would split a ZWJ emoji or a base+combining-mark
/// cluster apart, and a hardcoded `width: 1` would mislabel a CJK cluster's
/// true 2-cell width, both of which `blit`'s own wide-cell handling (and
/// ratatui's buffer diffing beneath it) depend on being correct. Centering
/// is likewise computed from `text`'s DISPLAY width in cells
/// (`crate::width::display_width`), not its cluster or byte count. A `text`
/// wider than `width` is left-aligned and simply clipped by `blit`'s own
/// area bound rather than truncated here — `blit` already advances by each
/// cell's real width and stops at the area's right edge, so no cell here
/// ever gets written past the reserved columns.
fn centered_cells(text: &str, width: usize) -> Vec<Cell> {
    let text_width = display_width(text);
    let pad = width.saturating_sub(text_width) / 2;
    let mut cells = Vec::with_capacity(width);
    for _ in 0..pad {
        cells.push(blank_cell());
    }
    for grapheme in text.graphemes(true) {
        let w = grapheme_width(grapheme);
        cells.push(Cell {
            text: grapheme.to_string(),
            width: w as u8,
            style: Style::default(),
            buf_offset: -1,
        });
    }
    let used = pad + text_width;
    for _ in used..width {
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
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
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
            next_generation: 0,
        });
        app
    }

    #[test]
    fn info_card_lines_include_name_dims_and_kitty_reason() {
        let mut app = app_with_image_doc(true, ImageStatus::Pending);
        if let Some(image) = app.active_doc_mut().image.as_mut() {
            image.in_flight = Some(1);
        }
        let doc = app.doc(app.active).expect("doc");
        let lines = info_card_lines(&app, doc);
        assert!(lines.iter().any(|l| l == "x.png"));
        assert!(lines.iter().any(|l| l.contains("64x48")));
        assert!(lines.iter().any(|l| l.contains("decoding")));
    }

    /// `Pending` splits on `in_flight`: a decode actually running reads
    /// "decoding…", while one that was never scheduled — or whose reply was
    /// lost — says so and names the way out. Collapsing both into
    /// "decoding…" is what made a wedged decode look identical to a slow one
    /// for an entire session.
    #[test]
    fn pending_without_an_in_flight_decode_reads_differently_from_a_running_one() {
        let mut app = app_with_image_doc(true, ImageStatus::Pending);

        let not_scheduled = {
            let doc = app.doc(app.active).expect("doc");
            reason_line(&app, doc)
        };
        assert!(
            !not_scheduled.contains("decoding"),
            "a Pending image with no decode in flight must not claim to be decoding: \
             {not_scheduled:?}"
        );
        assert!(
            not_scheduled.contains("reload"),
            "it must name the recovery available to the user: {not_scheduled:?}"
        );

        if let Some(image) = app.active_doc_mut().image.as_mut() {
            image.in_flight = Some(7);
        }
        let running = {
            let doc = app.doc(app.active).expect("doc");
            reason_line(&app, doc)
        };
        assert!(
            running.contains("decoding"),
            "a genuinely in-flight decode must still read as decoding: {running:?}"
        );
        assert_ne!(
            running, not_scheduled,
            "the two Pending states must be distinguishable on screen"
        );
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

    fn doc_row_ref(row: usize, width: usize) -> ImageRowRef {
        ImageRowRef {
            row,
            width,
            target: None,
        }
    }

    #[test]
    fn row_cells_center_the_requested_line() {
        let app = app_with_image_doc(true, ImageStatus::Pending);
        let doc = app.doc(app.active).expect("doc");
        let cells =
            row_cells(&app, doc, doc_row_ref(0, 0), 20).expect("doc-image row is always Some");
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
        let cells =
            row_cells(&app, doc, doc_row_ref(99, 0), 10).expect("doc-image row is always Some");
        assert!(cells.iter().all(|c| c.text == " "));
    }

    fn embed_row_ref(row: usize, width: usize, target: &str) -> ImageRowRef {
        ImageRowRef {
            row,
            width,
            target: Some(target.to_string()),
        }
    }

    #[test]
    fn a_live_embed_renders_placeholder_cells_with_a_left_margin_and_the_allocated_id() {
        let mut app = app_with_image_doc(true, ImageStatus::Pending);
        app.doc_mut(app.active).expect("doc").embeds.images.insert(
            "x.png".to_string(),
            crate::graphics::EmbedState {
                abs_path: PathBuf::from("/vault/x.png"),
                id: 0x00_10_20,
                mtime: None,
                dims: Some((64, 48)),
                cells: Some((8, 3)),
                decoded: None,
                status: ImageStatus::Live,
                in_flight: None,
            },
        );
        let doc = app.doc(app.active).expect("doc");
        let cells = row_cells(&app, doc, embed_row_ref(0, 8, "x.png"), 20)
            .expect("a live, Kitty-capable embed row is Some");
        assert_eq!(cells.len(), 20);
        assert_eq!(cells[0].text, " ", "one blank left-margin cell first");
        assert_eq!(
            cells[1].style.fg,
            Some(Color::Rgb(0x00, 0x10, 0x20)),
            "the second cell carries the allocated id"
        );
    }

    #[test]
    fn an_embed_with_kitty_unavailable_falls_through_to_alt_text() {
        let app = app_with_image_doc(false, ImageStatus::Pending);
        let doc = app.doc(app.active).expect("doc");
        assert!(row_cells(&app, doc, embed_row_ref(0, 8, "x.png"), 20).is_none());
    }

    #[test]
    fn an_embed_not_yet_live_reserves_blank_cells() {
        let app = app_with_image_doc(true, ImageStatus::Pending);
        let doc = app.doc(app.active).expect("doc");
        let cells = row_cells(&app, doc, embed_row_ref(0, 8, "untracked.png"), 20)
            .expect("Kitty-capable, not-yet-live embed row still reserves blanks");
        assert!(cells.iter().all(|c| c.text == " "));
    }

    #[test]
    fn human_size_formats_bytes_and_larger_units() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1536), "1.5 KB");
    }

    /// `centered_cells` must build ONE `Cell` per grapheme CLUSTER, at that
    /// cluster's real `grapheme_width`, never one `Cell` per `char` at a
    /// hardcoded width — the file names it renders (the card's own name
    /// line, an inline embed's link target) are arbitrary user text, not
    /// ASCII. `text` here is assumed to contain no space, so every
    /// non-space cell belongs to `text` and every space cell is centering
    /// padding.
    fn non_blank_cells(cells: &[Cell]) -> Vec<&Cell> {
        cells.iter().filter(|c| c.text != " ").collect()
    }

    #[test]
    fn centered_cells_segments_a_cjk_filename_by_grapheme_not_char() {
        let text = "\u{753b}\u{50cf}.png"; // "画像.png"
        let expected: Vec<&str> = text.graphemes(true).collect();
        let width = 20;
        let cells = centered_cells(text, width);

        assert_eq!(cells.iter().map(|c| c.width as usize).sum::<usize>(), width);

        let content = non_blank_cells(&cells);
        assert_eq!(
            content.len(),
            expected.len(),
            "one Cell per grapheme cluster"
        );
        for (cell, cluster) in content.iter().zip(expected.iter()) {
            assert_eq!(cell.text, *cluster);
            assert_eq!(cell.width as usize, grapheme_width(cluster));
            assert_eq!(cell.buf_offset, -1);
        }

        let text_width = display_width(text);
        let expected_pad = (width - text_width) / 2;
        let leading_pad = cells.iter().take_while(|c| c.text == " ").count();
        assert_eq!(
            leading_pad, expected_pad,
            "centering pad is measured in cells, not chars"
        );
    }

    #[test]
    fn centered_cells_keeps_a_zwj_family_emoji_as_one_cluster() {
        // "man + ZWJ + woman + ZWJ + girl + ZWJ + boy" — a single extended
        // grapheme cluster despite being seven code points.
        let family = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}\u{200d}\u{1f466}";
        let text = format!("{family}.png");
        let expected: Vec<&str> = text.graphemes(true).collect();
        assert_eq!(
            expected[0], family,
            "the ZWJ sequence must stay one cluster"
        );

        let width = 20;
        let cells = centered_cells(&text, width);

        assert_eq!(cells.iter().map(|c| c.width as usize).sum::<usize>(), width);

        let content = non_blank_cells(&cells);
        assert_eq!(
            content.len(),
            expected.len(),
            "one Cell per grapheme cluster"
        );
        assert_eq!(
            content[0].text, family,
            "the family emoji is never split apart"
        );
        assert_eq!(
            content[0].width as usize,
            grapheme_width(family),
            "its width is the cluster's own grapheme_width, not a per-char sum"
        );
    }

    #[test]
    fn centered_cells_keeps_a_base_plus_combining_mark_as_one_cluster() {
        // "cafe" + combining acute accent (NFD "café").
        let text = "cafe\u{0301}.png";
        let expected: Vec<&str> = text.graphemes(true).collect();
        assert_eq!(
            expected.len(),
            8,
            "the base+mark pair is one cluster, not two"
        );

        let width = 20;
        let cells = centered_cells(text, width);

        assert_eq!(cells.iter().map(|c| c.width as usize).sum::<usize>(), width);

        let content = non_blank_cells(&cells);
        assert_eq!(
            content.len(),
            expected.len(),
            "one Cell per grapheme cluster"
        );
        for (cell, cluster) in content.iter().zip(expected.iter()) {
            assert_eq!(cell.text, *cluster);
            assert_eq!(cell.width as usize, grapheme_width(cluster));
        }
    }
}
