//! WP5.S7: `Msg::Highlighted` semantics — keep-on-`None` (`[R2]`),
//! drop-on-stale-version, and clamp-plus-char-boundary-discard on receipt
//! (§1.3) — plus one end-to-end highlight-then-render check. The painter
//! resolution itself (a hand-built `(rows, spans)` pair, outer-first
//! overwrite, `buf_offset`/`width` left untouched) is unit-tested inside
//! `render/overlay.rs`'s own `#[cfg(test)]` module instead: its target,
//! `apply_highlight_spans`, is `pub(super)` like every other overlay
//! function in that file (`apply_cursor_overlays`, `highlight_selection`,
//! `place_caret`), so it is unreachable from this external integration test
//! crate — only the crate's own public surface (`app::update`, `Document`,
//! `render::build_rows`/`testgrid`) is.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use rune_core::buffer::Buffer;
use rune_syntax::scope::scope_table;
use rune_tui::app::{self, App};
use rune_tui::runtime::{Effects, Msg};
use rune_tui::testgrid;
use rune_vfs::Mem;

fn app_for(content: &str, path: &str) -> App {
    App::new(
        Buffer::new(content),
        Some(PathBuf::from(path)),
        Arc::new(Mem::new()),
        None,
    )
}

#[test]
fn none_result_leaves_spans_byte_identical() {
    let mut app = app_for("fn main() {}\n", "/x/main.rs");
    let id = app.active;
    let keyword = scope_table().resolve("keyword").expect("known scope");
    let before = vec![(0..2, keyword)];
    app.doc_mut(id).expect("doc").highlight.spans = before.clone();
    let version = app.doc(id).expect("doc").buffer.version();

    let mut effects = Effects::default();
    app::update(
        &mut app,
        Msg::Highlighted {
            doc: id,
            version,
            result: None,
        },
        &mut effects,
    );

    assert_eq!(app.doc(id).expect("doc").highlight.spans, before);
}

#[test]
fn reply_at_a_stale_version_leaves_spans_unchanged() {
    let mut app = app_for("fn main() {}\n", "/x/main.rs");
    let id = app.active;
    let keyword = scope_table().resolve("keyword").expect("known scope");
    let before = vec![(0..2, keyword)];
    app.doc_mut(id).expect("doc").highlight.spans = before.clone();
    let stale_version = app.doc(id).expect("doc").buffer.version();

    // Advance the buffer past `stale_version` without going through a real
    // edit command — a direct field write is the same convention
    // `tests/tui_render.rs::app_for` already uses for other `Document`
    // fields (`cursors`, `viewport`).
    {
        let doc = app.doc_mut(id).expect("doc");
        doc.buffer = doc.buffer.insert(0, "x");
    }

    let mut effects = Effects::default();
    app::update(
        &mut app,
        Msg::Highlighted {
            doc: id,
            version: stale_version,
            result: Some(vec![(0..3, keyword)]),
        },
        &mut effects,
    );

    assert_eq!(app.doc(id).expect("doc").highlight.spans, before);
}

#[test]
// The `5..3` payload below is a deliberately inverted range — exactly the
// hostile reply `handle_highlighted` must discard — not an accidental
// reversed-iteration mistake.
#[allow(clippy::reversed_empty_ranges)]
fn clamps_and_drops_out_of_bounds_and_off_char_boundary_ranges() {
    // 3 CJK codepoints (3 bytes each) + `\n` = 10 bytes; byte 1 sits inside
    // the first codepoint, never on a `char` boundary.
    let content = "日本語\n";
    let mut app = app_for(content, "/x/main.rs");
    let id = app.active;
    let len = app.doc(id).expect("doc").buffer.content().len();
    assert_eq!(len, 10);
    let version = app.doc(id).expect("doc").buffer.version();
    let keyword = scope_table().resolve("keyword").expect("known scope");

    let mut effects = Effects::default();
    app::update(
        &mut app,
        Msg::Highlighted {
            doc: id,
            version,
            result: Some(vec![
                (0..1000, keyword), // past the end -> clamped to `len`
                (5..3, keyword),    // inverted -> dropped
                (1..2, keyword),    // mid-char -> dropped
                (0..3, keyword),    // valid, char-boundary aligned
            ]),
        },
        &mut effects,
    );

    let doc = app.doc(id).expect("doc");
    let content = doc.buffer.content();
    for (range, _) in &doc.highlight.spans {
        assert!(range.start < range.end);
        assert!(range.end <= content.len());
        assert!(content.is_char_boundary(range.start));
        assert!(content.is_char_boundary(range.end));
    }
    assert!(doc.highlight.spans.contains(&(0..len, keyword)));
    assert!(doc.highlight.spans.contains(&(0..3, keyword)));
    assert_eq!(
        doc.highlight.spans.len(),
        2,
        "the inverted and mid-char ranges must be dropped"
    );
}

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
            result,
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
