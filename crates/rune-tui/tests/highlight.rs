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
//!
//! WP6.S5 adds the fenced-code-in-markdown end-to-end cases below: a
//! markdown document's own fences schedule and apply through the SAME
//! `Msg::Highlighted`/`update`/`testgrid` path as a whole code document —
//! `crate::highlight::schedule_highlight`, `fence_language` and
//! `code_fence_sources` are private to `rune-tui`, so these drive the real
//! public chokepoints (`app::update`, `Cmd::run`) instead of calling them
//! directly. `clippy::panic` joins the allow list here (matching
//! `tests/opentabs.rs`/`tests/db_wiring.rs`/`tests/rename.rs`/
//! `tests/banner.rs`'s own convention) for the "wrong Msg variant landed"
//! assertions those new cases need.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use rune_core::buffer::Buffer;
use rune_core::cursor::CursorSet;
use rune_syntax::scope::scope_table;
use rune_tui::app::{self, App};
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
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

/// Types one harmless character at the END of the active document's buffer
/// (plan WP6.S5) through the real `app::update` chokepoint, bumping its
/// buffer version so `App::update`'s own before/after gate schedules a
/// highlight `Cmd` — mirrors how a real keystroke does it, without needing
/// the private `highlight::schedule_highlight` directly. Positioning the
/// cursor at the very end first (rather than wherever `App::new` put it)
/// means the appended byte lands after every fence in the fixtures below,
/// so it never shifts a fence's own byte range out from under the parse
/// `code_fences()` reads (that parse is refreshed by `App::sync_view`, not
/// by this call — calling `sync_view` once before this, as every test below
/// does, mirrors `runtime::run`'s own bootstrap ordering).
fn type_one_char_at_end(app: &mut App, effects: &mut Effects) {
    let id = app.active;
    let end = app.doc(id).expect("doc").buffer.content().len();
    app.doc_mut(id).expect("doc").cursors = CursorSet::new(end);
    app::update(
        app,
        Msg::Key(KeyInput {
            code: KeyCode::Char('!'),
            mods: Mods::NONE,
        }),
        effects,
    );
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

/// Plan WP6.S5, bullet 1: a markdown document with one ```` ```rust ````
/// fence produces at least one stored span inside the fence's own content
/// bytes and none outside it.
#[test]
fn markdown_rust_fence_produces_spans_inside_the_fence_only() {
    let content = "Intro paragraph.\n\n```rust\nfn main() {}\n```\n\nOutro.\n";
    let mut app = app_for(content, "/x/notes.md");
    // Mirrors `runtime::run`'s own bootstrap ordering (plan Context, "the
    // async seam"): `DocMachine::code_fences` reads the LAST parse
    // `sync_view` produced, not the live buffer, so a fence must have been
    // parsed at least once before an edit can find it.
    app.sync_view();

    let mut effects = Effects::default();
    type_one_char_at_end(&mut app, &mut effects);

    assert_eq!(
        effects.cmds.len(),
        1,
        "the rust fence must schedule exactly one highlight cmd"
    );
    let msg = effects
        .cmds
        .remove(0)
        .run()
        .expect("fence_highlight_cmd always replies with Some(Msg::Highlighted)");
    let Msg::Highlighted { result, .. } = &msg else {
        panic!("expected a Msg::Highlighted reply, got {msg:?}");
    };
    let spans = result
        .clone()
        .expect("the rust fence must parse within the budget");
    assert!(!spans.is_empty());

    let fence_start = content.find("fn main").expect("fixture has a fence body");
    let fence_end = content
        .find("```\n\nOutro")
        .expect("fixture has a fence close");
    for (range, _) in &spans {
        assert!(
            range.start >= fence_start && range.end <= fence_end,
            "span {range:?} escapes the fence content bytes {fence_start}..{fence_end}"
        );
    }

    let mut effects2 = Effects::default();
    app::update(&mut app, msg, &mut effects2);
    let doc = app.doc(app.active).expect("doc");
    assert!(!doc.highlight.spans.is_empty());
    for (range, _) in &doc.highlight.spans {
        assert!(range.start >= fence_start && range.end <= fence_end);
    }
}

/// Plan WP6.S5, bullet 2: rendering a fenced markdown document leaves
/// every cell inside the fence carrying a non-`None` background — the
/// `markup.raw.block` fence background survives the highlight overlay's
/// `patch` (plan decision 2), since `code_scope_style` sets only `fg` and
/// never `.bg(..)` (WP1.S4).
#[test]
fn fence_background_survives_the_highlight_overlay_patch() {
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
    let raw_block_bg = app
        .theme
        .scope_style(
            scope_table()
                .resolve("markup.raw.block")
                .expect("known scope"),
        )
        .bg;
    assert!(
        raw_block_bg.is_some(),
        "the fence background style itself must carry a bg"
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
                    raw_block_bg,
                    "fence content cell at column {x} lost its markup.raw.block background"
                );
            }
        }
    }
    assert!(found_fence_row, "the fence content row must be on screen");
}

/// Plan WP6.S5, bullet 3: a fence tagged ```` ```rust,ignore ```` still
/// resolves to `rust` (info string split on whitespace AND `,`, first
/// token only) and still produces spans.
#[test]
fn fence_tagged_rust_comma_ignore_still_highlights() {
    let content = "```rust,ignore\nfn main() {}\n```\n";
    let mut app = app_for(content, "/x/notes.md");
    app.sync_view();

    let mut effects = Effects::default();
    type_one_char_at_end(&mut app, &mut effects);

    assert_eq!(
        effects.cmds.len(),
        1,
        "rust,ignore must still resolve and schedule a highlight cmd"
    );
    let msg = effects.cmds.remove(0).run().expect("reply always arrives");
    let Msg::Highlighted { result, .. } = &msg else {
        panic!("expected a Msg::Highlighted reply, got {msg:?}");
    };
    assert!(
        result.as_ref().is_some_and(|spans| !spans.is_empty()),
        "a rust,ignore fence must still produce spans"
    );
}

/// Plan WP6.S5, bullet 4: an unknown fence tag and an untagged fence each
/// produce zero spans and no error — neither resolves through
/// `rune_ts::lang::resolve`, so `code_fence_sources` returns nothing and
/// `schedule_highlight`'s markdown branch schedules no `Cmd` at all.
#[test]
fn unknown_and_untagged_fences_schedule_nothing() {
    let content = "```klingon\nQapla'\n```\n\n```\nplain fenced text\n```\n";
    let mut app = app_for(content, "/x/notes.md");
    app.sync_view();

    let mut effects = Effects::default();
    type_one_char_at_end(&mut app, &mut effects);

    assert!(
        effects.cmds.is_empty(),
        "no fence resolved to a known language, so no highlight cmd should be scheduled"
    );
    assert!(app.doc(app.active).expect("doc").highlight.spans.is_empty());
}
