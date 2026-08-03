//! Split off `highlight.rs` (WP11, §1.6): end-to-end highlight-then-render
//! checks. The painter resolution itself (a hand-built `(rows, spans)`
//! pair, outer-first overwrite, `buf_offset`/`width` left untouched) is
//! unit-tested inside `render/overlay.rs`'s own `#[cfg(test)]` module
//! instead: its target, `apply_highlight_spans`, is `pub(super)` like every
//! other overlay function in that file (`apply_cursor_overlays`,
//! `highlight_selection`, `place_caret`), so it is unreachable from this
//! external integration test crate — only the crate's own public surface
//! (`app::update`, `Document`, `render::build_rows`/`testgrid`) is.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod highlight_common;

use std::time::Duration;

use highlight_common::{app_for, type_one_char_at_end};
use rune_syntax::scope::scope_table;
use rune_tui::app;
use rune_tui::runtime::{Effects, HighlightPayload, Msg};
use rune_tui::testgrid;

#[test]
fn a_real_highlight_reply_colours_a_code_document_without_changing_its_text() {
    let content = "fn main() {}\n";
    let mut app = app_for(content, "/x/main.rs");
    let id = app.active;
    app.doc_mut(id).expect("doc").viewport.set_size(40, 10);
    let version = app.doc(id).expect("doc").buffer.version();

    let result = rune_ts::highlight("rust", content, Duration::from_secs(5));
    assert!(
        result.is_some(),
        "a trivial rust source must highlight within a generous budget"
    );

    let mut effects = Effects::default();
    app::update(
        &mut app,
        Msg::Highlighted {
            doc: id,
            version,
            result: result.map(HighlightPayload::Spans),
        },
        &mut effects,
    );

    app.sync_view();
    let rendered = testgrid::grid(&app, 40, 10).join("\n");
    assert!(
        rendered.contains("fn main() {}"),
        "the overlay must never change the rendered TEXT, only cell styles"
    );

    let buf = testgrid::draw(&app, 40, 10);
    let plain = app
        .theme
        .scope_style(scope_table().resolve("text").expect("known scope"));
    let mut any_non_plain = false;
    for y in 0..10 {
        for x in 0..40 {
            if let Some(cell) = buf.cell((x, y))
                && cell.style().fg != plain.fg
            {
                any_non_plain = true;
            }
        }
    }
    assert!(
        any_non_plain,
        "at least one cell must carry a token colour distinct from the plain text scope"
    );
}

/// An overlay must PATCH a cell's style, never replace it: rendering a
/// fenced markdown document leaves every cell inside the fence still
/// carrying the code region's background rectangle after the highlight
/// overlay has painted token foregrounds over it. `code_scope_style` sets
/// only `fg` and never `.bg(..)`, and `Theme::overlay_scope_style` strips
/// any `bg` a scope might carry, which together are what make that true.
#[test]
fn code_background_survives_the_highlight_overlay_patch() {
    let content = "Intro paragraph.\n\n```rust\nfn main() {}\n```\n\nOutro.\n";
    let mut app = app_for(content, "/x/notes.md");
    app.sync_view();
    app.doc_mut(app.active)
        .expect("doc")
        .viewport
        .set_size(40, 12);

    let mut effects = Effects::default();
    type_one_char_at_end(&mut app, &mut effects);
    let msg = effects
        .cmds
        .remove(0)
        .run()
        .expect("fence_highlight_cmd always replies");
    let mut effects2 = Effects::default();
    app::update(&mut app, msg, &mut effects2);

    app.sync_view();
    let buf = testgrid::draw(&app, 40, 12);
    let code_bg = Some(app.theme.chrome.code_bg);
    assert_eq!(
        app.theme
            .scope_style(
                scope_table()
                    .resolve("markup.raw.block")
                    .expect("known scope"),
            )
            .bg,
        None,
        "the code background is a region rectangle, never a span bg"
    );

    // Scoped to the fence TEXT's own columns, not the whole row: `geo.
    // center_bordered`'s rounded-border box shares the row with the
    // editor's content on a narrow frame, and that border cell's style is
    // chrome, not `markup.raw.block` — checking the whole row would assert
    // an unrelated cell instead of the fence's own. Matched cell-by-cell
    // (not via `String::find`) because the border glyph is a multi-BYTE
    // UTF-8 char while a terminal column is one CELL — `find`'s byte
    // offset and the column index silently diverge the moment a
    // multi-byte cell precedes the match.
    let needle: Vec<char> = "fn main() {}".chars().collect();
    let mut found_fence_row = false;
    for y in 0..12u16 {
        for x0 in 0..40u16 {
            let matched = needle.iter().enumerate().all(|(k, &nc)| {
                let x = x0 + u16::try_from(k).unwrap_or(u16::MAX);
                buf.cell((x, y))
                    .is_some_and(|cell| cell.symbol() == nc.to_string())
            });
            if !matched {
                continue;
            }
            found_fence_row = true;
            for k in 0..needle.len() {
                let x = x0 + u16::try_from(k).unwrap_or(u16::MAX);
                let cell = buf.cell((x, y)).expect("just matched above");
                assert_eq!(
                    cell.style().bg,
                    code_bg,
                    "fence content cell at column {x} lost the code region's background"
                );
            }
        }
    }
    assert!(found_fence_row, "the fence content row must be on screen");
}
