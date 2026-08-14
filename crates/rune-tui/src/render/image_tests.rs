//! Unit tests for the image info-card row builder, kept in a sibling file
//! so `image.rs` itself stays inside the 500-line budget.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_vfs::Mem;

use super::*;
use crate::graphics::ImageState;

const X_PNG: &[u8] = include_bytes!("../../../../testdata/assets/x.png");

fn decoded_fixture() -> rune_image::decode::Decoded {
    rune_image::decode_still(X_PNG).expect("decode x.png")
}

fn app_with_kitty(kitty: bool) -> App {
    let mut app = App::new(Buffer::new(""), None, Arc::new(Mem::new()), None);
    app.graphics.kitty = kitty;
    app
}

fn app_with_image_doc(kitty: bool, status: ImageStatus) -> App {
    let mut app = App::new(Buffer::new(""), None, Arc::new(Mem::new()), None);
    app.graphics.kitty = kitty;
    let id = app.active;
    let doc = app.doc_mut(id).expect("doc");
    doc.bind_path(PathBuf::from("/vault/x.png"));
    doc.read_only = crate::document::ReadOnly::Always;
    doc.display_name = Some("x.png".to_string());
    doc.set_image(ImageState {
        path: PathBuf::from("/vault/x.png"),
        bytes_len: 146,
        id: 1,
        dims: Some(rune_image::PixelSize { w: 64, h: 48 }),
        status,
        in_flight: None,
        next_generation: 0,
    });
    app
}

#[test]
fn info_card_lines_include_name_dims_and_kitty_reason() {
    let mut app = app_with_image_doc(true, ImageStatus::Pending);
    if let Some(image) = app.active_doc_mut().image_mut() {
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

    if let Some(image) = app.active_doc_mut().image_mut() {
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
    let app = app_with_image_doc(
        false,
        ImageStatus::Live {
            decoded: decoded_fixture(),
            cells: rune_image::CellFootprint { cols: 8, rows: 3 },
        },
    );
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
    let cells = row_cells(&app, doc, &doc_row_ref(0, 0), 20).expect("doc-image row is always Some");
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
        row_cells(&app, doc, &doc_row_ref(99, 0), 10).expect("doc-image row is always Some");
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
    let mut app = app_with_kitty(true);
    app.doc_mut(app.active)
        .expect("doc")
        .ensure_embeds()
        .expect("markdown document")
        .images
        .insert(
            "x.png".to_string(),
            crate::graphics::EmbedState {
                abs_path: PathBuf::from("/vault/x.png"),
                id: 0x00_10_20,
                mtime: None,
                dims: Some(rune_image::PixelSize { w: 64, h: 48 }),
                status: ImageStatus::Live {
                    decoded: decoded_fixture(),
                    cells: rune_image::CellFootprint { cols: 8, rows: 3 },
                },
                in_flight: None,
            },
        );
    let doc = app.doc(app.active).expect("doc");
    let cells = row_cells(&app, doc, &embed_row_ref(0, 8, "x.png"), 20)
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
    let app = app_with_kitty(false);
    let doc = app.doc(app.active).expect("doc");
    assert!(row_cells(&app, doc, &embed_row_ref(0, 8, "x.png"), 20).is_none());
}

#[test]
fn an_embed_not_yet_live_reserves_blank_cells() {
    let app = app_with_kitty(true);
    let doc = app.doc(app.active).expect("doc");
    let cells = row_cells(&app, doc, &embed_row_ref(0, 8, "untracked.png"), 20)
        .expect("Kitty-capable, not-yet-live embed row still reserves blanks");
    assert!(cells.iter().all(|c| c.text == " "));
}

#[test]
fn human_size_formats_bytes_and_larger_units() {
    assert_eq!(human_size(512), "512 B");
    assert_eq!(human_size(1536), "1.5 KB");
}

/// An independent width oracle (matching `breadcrumb.rs`'s own
/// `oracle_cell_width` and the test-only `unicode-width` dev-dependency
/// `Cargo.toml` documents): computed straight from `unicode_width`, never
/// by calling this module's own `grapheme_width`/`push_grapheme_cells`, so
/// a regression in the production width math can't pass a test that merely
/// re-invokes the same broken function.
fn oracle_grapheme_width(cluster: &str) -> usize {
    cluster
        .chars()
        .filter_map(unicode_width::UnicodeWidthChar::width)
        .max()
        .unwrap_or(0)
        .max(1)
}

/// `centered_cells` must build ONE `Cell` per grapheme CLUSTER, at that
/// cluster's real display width, never one `Cell` per `char` at a
/// hardcoded width — the file names it renders (the card's own name
/// line, an inline embed's link target) are arbitrary user text, not
/// ASCII. Slices out `text`'s own cells STRUCTURALLY, by the same leading-
/// pad width `centered_cells` itself computes and documents (`(width -
/// display_width(text)) / 2`) followed by one cell per grapheme cluster in
/// `text` — never by filtering out `" "` cells, which would misidentify a
/// space that is itself part of `text` as padding.
fn content_cells<'a>(cells: &'a [Cell], text: &str, width: usize) -> &'a [Cell] {
    let pad = width.saturating_sub(display_width(text)) / 2;
    let content_len = text.graphemes(true).count();
    &cells[pad..pad + content_len]
}

#[test]
fn centered_cells_segments_a_cjk_filename_by_grapheme_not_char() {
    let text = "\u{753b}\u{50cf}.png"; // "画像.png"
    let expected: Vec<&str> = text.graphemes(true).collect();
    let width = 20;
    let cells = centered_cells(text, width);

    assert_eq!(cells.iter().map(|c| c.width as usize).sum::<usize>(), width);

    let content = content_cells(&cells, text, width);
    assert_eq!(
        content.len(),
        expected.len(),
        "one Cell per grapheme cluster"
    );
    for (cell, cluster) in content.iter().zip(expected.iter()) {
        assert_eq!(cell.text, *cluster);
        assert_eq!(cell.width as usize, oracle_grapheme_width(cluster));
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

    let content = content_cells(&cells, &text, width);
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
        oracle_grapheme_width(family),
        "its width is the cluster's own display width, not a per-char sum"
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

    let content = content_cells(&cells, text, width);
    assert_eq!(
        content.len(),
        expected.len(),
        "one Cell per grapheme cluster"
    );
    for (cell, cluster) in content.iter().zip(expected.iter()) {
        assert_eq!(cell.text, *cluster);
        assert_eq!(cell.width as usize, oracle_grapheme_width(cluster));
    }
}

/// The control-character exposure Finding 2 fixed: `centered_cells` is fed
/// arbitrary user text (a file name, an inline embed's link target), which
/// can contain a raw ASCII control byte. Before this fix, `centered_cells`
/// built its own `Cell`s directly and handed ratatui a literal control byte
/// as a cell's `text` — ratatui-core's `cell_width()` asserts a
/// single-byte symbol is never `is_ascii_control` and panics the instant
/// that cell is diffed. Routing through `push_grapheme_cells` (the same
/// chokepoint `segment_cells` uses for real buffer content) substitutes the
/// safe control-picture glyph instead, so a tab or a bare `\x07` in a file
/// name can never reach ratatui unfiltered.
#[test]
fn centered_cells_substitutes_a_control_character_instead_of_passing_it_through() {
    let text = "bad\u{7}name.png"; // a bare BEL control byte
    let width = 20;
    let cells = centered_cells(text, width);

    assert_eq!(cells.iter().map(|c| c.width as usize).sum::<usize>(), width);
    for cell in &cells {
        assert!(
            cell.text.chars().all(|c| !c.is_ascii_control()),
            "no cell may carry a raw ASCII control byte: {cell:?}"
        );
    }
    let text: String = cells.iter().map(|c| c.text.as_str()).collect();
    assert!(
        text.contains('\u{2407}'),
        "the control byte is replaced with its safe placeholder glyph: {text:?}"
    );
}

/// The width-drift Finding 2's fix could otherwise reintroduce: a TAB is
/// the one grapheme whose expansion width is NOT a fixed per-cluster
/// number — `crate::width::display_width` (a prior version's pad source)
/// gives it width 1 (`control_aware_width`'s single-char clamp), while
/// `push_grapheme_cells` (what actually builds the returned cells) expands
/// it to the next 4-column tab stop. Measuring `pad` from one function and
/// building cells from the other let the declared row overrun `width`
/// silently. `centered_cells` now derives `pad` from the SAME chokepoint
/// it builds with, so this must hold for text containing a tab exactly as
/// it does for plain text.
#[test]
fn centered_cells_with_a_tab_still_declares_exactly_width() {
    let text = "a\tb.png";
    for width in [10, 20, 21, 30] {
        let cells = centered_cells(text, width);
        assert_eq!(
            cells.iter().map(|c| c.width as usize).sum::<usize>(),
            width,
            "a tab in the text must never make the row over- or under-run \
             its reserved width {width}"
        );
    }
}
