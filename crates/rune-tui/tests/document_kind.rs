//! WP4.S6: `Document::bind_path` derives `DocumentKind` from the extension
//! via `rune_ts::lang::resolve` — never `registry()` (plan `[B5]`) — and a
//! code document renders its source verbatim (no comrak parse, no
//! concealment at all), while a `.md` path keeps today's markdown pipeline.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_core::cursor::CursorSet;
use rune_syntax::DocumentKind;
use rune_tui::app::App;
use rune_tui::testgrid;
use rune_vfs::Mem;

const WIDTH: u16 = 80;
const HEIGHT: u16 = 24;

fn app_for(content: &str, path: Option<&str>) -> App {
    let mut app = App::new(
        Buffer::new(content),
        path.map(PathBuf::from),
        Arc::new(Mem::new()),
        None,
    );
    let id = app.active;
    app.doc_mut(id)
        .expect("active document")
        .viewport
        .set_size(WIDTH, HEIGHT - 1);
    app.sync_view();
    app
}

fn grid_text(app: &App) -> String {
    testgrid::grid(app, WIDTH, HEIGHT).join("\n")
}

/// A `.rs` path becomes `DocumentKind::Code("rust")` and renders its source
/// literally: no comrak parse ran at all, so `# not a heading` is never
/// treated as a heading and never concealed — unlike the `.md` case below.
#[test]
fn rs_extension_is_code_rust_and_renders_verbatim() {
    let content = "# not a heading\nfn main() {}\n";
    let app = app_for(content, Some("/x/main.rs"));

    assert_eq!(app.active_doc().kind, DocumentKind::Code("rust"));

    let text = grid_text(&app);
    assert!(
        text.contains("# not a heading"),
        "a code document must render its source verbatim, marker and all:\n{text}"
    );
}

/// The same content under a `.md` path keeps `DocumentKind::Markdown` and
/// the existing comrak pipeline — including heading concealment when the
/// cursor sits elsewhere (mirrors `tui_render.rs`'s own concealment
/// fixture).
#[test]
fn md_extension_stays_markdown_and_still_conceals() {
    let content = "# not a heading\nfn main() {}\n";
    let mut app = app_for(content, Some("/x/notes.md"));

    assert_eq!(app.active_doc().kind, DocumentKind::Markdown);

    // Move the cursor off line 0 so the heading's `Decide` reveal policy
    // conceals its `# ` marker (plan Context, reveal-policy table).
    let cursor_offset = content.find("fn main").expect("fixture contains 'fn main'");
    let id = app.active;
    app.doc_mut(id).expect("active document").cursors = CursorSet::new(cursor_offset);
    app.sync_view();

    let text = grid_text(&app);
    assert!(
        !text.contains("# not a heading"),
        "a markdown document must still conceal the heading marker:\n{text}"
    );
    assert!(
        text.contains("not a heading"),
        "the heading's own text must still render:\n{text}"
    );
}

/// An unrecognised extension has no `lang::resolve` match, so the document
/// falls back to `DocumentKind::Plain` (rendered verbatim, no language).
#[test]
fn unknown_extension_is_plain() {
    let app = app_for("whatever bytes\n", Some("/x/data.bin"));
    assert_eq!(app.active_doc().kind, DocumentKind::Plain);
}

/// A pathless document (`App::new`'s `file_path: None`) — an untitled draft
/// — stays `DocumentKind::Markdown`, exactly as before this plan.
#[test]
fn no_path_stays_markdown() {
    let app = app_for("hello\n", None);
    assert_eq!(app.active_doc().kind, DocumentKind::Markdown);
}
