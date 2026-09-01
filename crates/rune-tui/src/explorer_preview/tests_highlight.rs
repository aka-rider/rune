#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
use rune_vfs::VfsTestExt;
use std::path::PathBuf;
use std::sync::Arc;

use rune_core::buffer::Buffer as CoreBuffer;
use rune_vfs::{Mem, Vfs};

use super::tests_common::{app_with, load_entries, run_cmds, run_cmds_through_update};
use super::*;
use crate::runtime::Msg;

fn live_db() -> crate::db::Db {
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Mem::new());
    let clock: rune_db::ClockFn = Arc::new(std::time::SystemTime::now);
    let store =
        rune_db::Store::open_in_memory(clock, vfs, Box::new(|_evt| {})).expect("open store");
    let bridge = crate::db::DbBridge::bootstrap();
    crate::db::Db::new(store, bridge, false)
}

fn preview_id(app: &App) -> crate::document::DocumentId {
    app.explorer.preview.as_ref().expect("preview minted").id
}

fn schedule_then_deliver_preview_highlight(app: &mut App, effects: &mut Effects) {
    run_cmds_through_update(app, effects);
    run_cmds_through_update(app, effects);
}

#[test]
fn promoting_the_preview_enqueues_recovery_store_hydration() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(std::path::Path::new("/root/a.md"), b"content")
        .unwrap();
    let mut app = app_with(&mem);
    app.db = Some(live_db());
    load_entries(&mut app, &["a.md"]);
    let mut effects = Effects::default();
    app.explorer.nav.move_by(1, app.explorer.entries.len());
    after_cursor_move(&mut app, &mut effects);
    run_cmds(&mut app, &mut effects);
    let id = preview_id(&app);
    app.set_focus_pane(Pane::Explorer, &mut effects);
    assert!(
        app.db_ops.is_empty(),
        "a preview never contacts the recovery store before promotion"
    );

    let escape = crate::keymap::KeyInput {
        code: crate::keymap::KeyCode::Escape,
        mods: crate::keymap::Mods::NONE,
    };
    crate::app::update(&mut app, Msg::Key(escape), &mut effects);

    assert!(
        app.db_ops
            .values()
            .any(|op| op.doc == id && op.issued_version.is_some()),
        "promotion must enqueue a Load op hydrating the document through the recovery store"
    );
}

#[test]
fn arrowing_across_files_schedules_a_highlight_for_every_preview_because_previews_are_never_active()
{
    let mem = Arc::new(Mem::new());
    let code = "```rust\nfn a() {}\n```\n";
    for name in ["a.md", "b.md", "c.md"] {
        mem.save_atomic(&PathBuf::from("/root").join(name), code.as_bytes())
            .unwrap();
    }
    let mut app = app_with(&mem);
    load_entries(&mut app, &["a.md", "b.md", "c.md"]);
    let mut effects = Effects::default();

    for n in 0..3 {
        app.explorer.nav.move_by(1, app.explorer.entries.len());
        after_cursor_move(&mut app, &mut effects);
        run_cmds_through_update(&mut app, &mut effects);

        let preview = app.explorer.preview.as_ref().expect("preview minted");
        assert!(
            preview.doc.highlight.in_flight.is_some(),
            "file #{n} must schedule its own highlight"
        );
    }
}

#[test]
fn a_preview_stores_the_spans_its_own_reply_carries() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(
        std::path::Path::new("/root/a.md"),
        b"```rust\nfn a() {}\n```\n",
    )
    .unwrap();
    let mut app = app_with(&mem);
    load_entries(&mut app, &["a.md"]);
    let mut effects = Effects::default();
    app.explorer.nav.move_by(1, app.explorer.entries.len());
    after_cursor_move(&mut app, &mut effects);
    schedule_then_deliver_preview_highlight(&mut app, &mut effects);

    let preview = app.explorer.preview.as_ref().expect("preview minted");
    assert!(
        !preview.doc.highlight.regions.is_empty(),
        "the preview's own fence must be highlighted"
    );
    assert!(preview.doc.highlight.in_flight.is_none());
}

#[test]
fn a_highlight_reply_for_the_previous_preview_file_cannot_paint_the_next_one() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(std::path::Path::new("/root/a.md"), b"# a\n")
        .unwrap();
    mem.save_atomic(std::path::Path::new("/root/b.md"), b"# b\n")
        .unwrap();
    let mut app = app_with(&mem);
    load_entries(&mut app, &["a.md", "b.md"]);
    let mut effects = Effects::default();

    app.explorer.nav.move_by(1, app.explorer.entries.len()); // "a.md"
    after_cursor_move(&mut app, &mut effects);
    run_cmds_through_update(&mut app, &mut effects);
    let stale = preview_id(&app);
    let stale_version = app
        .explorer
        .preview
        .as_ref()
        .expect("preview")
        .doc
        .buffer
        .version();

    app.explorer.nav.move_by(1, app.explorer.entries.len()); // "b.md"
    after_cursor_move(&mut app, &mut effects);
    run_cmds_through_update(&mut app, &mut effects);
    let live = preview_id(&app);
    assert_ne!(stale, live, "a new preview target is a new document");

    let scope = rune_syntax::scope::scope_table()
        .resolve("keyword")
        .expect("known scope");
    let marker_line = 0..4;
    crate::app::update(
        &mut app,
        Msg::Highlighted {
            doc: stale,
            version: stale_version,
            result: crate::highlight::PassOutcome::Replace(crate::highlight::HighlightReply {
                regions: vec![crate::highlight::RegionResult {
                    map: crate::linemap::LineMap::new("# a\n", vec![marker_line]),
                    outcome: crate::highlight::RegionOutcome::Replace(
                        crate::highlight::RegionPayload::Spans {
                            source: String::new(),
                            spans: vec![(0..4, scope)],
                        },
                    ),
                }],
                truncated: false,
            }),
        },
        &mut effects,
    );

    let preview = app.explorer.preview.as_ref().expect("preview");
    assert!(
        preview.doc.highlight.regions.is_empty(),
        "a reply computed for the file the Explorer used to show must never install \
         onto the file that replaced it"
    );
}

#[test]
fn focus_entering_the_tabs_pane_discards_the_live_preview() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(std::path::Path::new("/root/a.md"), b"content")
        .unwrap();
    let mut app = app_with(&mem);
    let real = app.active;
    load_entries(&mut app, &["a.md"]);
    let mut effects = Effects::default();
    app.explorer.nav.move_by(1, app.explorer.entries.len());
    after_cursor_move(&mut app, &mut effects);
    run_cmds(&mut app, &mut effects);
    assert!(app.explorer.preview.is_some(), "preview minted");

    let ctrl_t = crate::keymap::KeyInput {
        code: crate::keymap::KeyCode::Char('t'),
        mods: crate::keymap::Mods {
            shift: false,
            alt: false,
            ctrl: true,
            sup: false,
        },
    };
    crate::app::update(&mut app, Msg::Key(ctrl_t), &mut effects);

    assert!(app.explorer.preview.is_none());
    assert_eq!(app.active, real, "the real tab was never left");
    assert_eq!(app.focus(), Pane::Tabs);
}

#[test]
fn discarding_a_preview_via_ctrl_digit_selects_exactly_the_named_tab() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(std::path::Path::new("/root/a.md"), b"content")
        .unwrap();
    let mut app = app_with(&mem);
    app.open_document(CoreBuffer::new("second"));
    let third = app.open_document(CoreBuffer::new("third"));
    workspace::switch_to(&mut app, third);
    load_entries(&mut app, &["a.md"]);
    let mut effects = Effects::default();
    app.explorer.nav.move_by(1, app.explorer.entries.len());
    after_cursor_move(&mut app, &mut effects);
    run_cmds(&mut app, &mut effects);
    assert!(app.explorer.preview.is_some(), "preview minted");
    let third_index = app
        .documents
        .order()
        .iter()
        .position(|&t| t == third)
        .expect("third tab is open");

    let ctrl_digit = crate::keymap::KeyInput {
        code: crate::keymap::KeyCode::Char(
            char::from_digit((third_index as u32 + 1) % 10, 10).expect("valid digit"),
        ),
        mods: crate::keymap::Mods {
            shift: false,
            alt: false,
            ctrl: true,
            sup: false,
        },
    };
    crate::app::update(&mut app, Msg::Key(ctrl_digit), &mut effects);

    assert!(app.explorer.preview.is_none());
    assert_eq!(app.active, third, "selecting the tab by digit lands on it");
}

#[test]
fn a_highlight_reply_still_reaches_its_document_after_the_preview_is_promoted() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(
        std::path::Path::new("/root/a.md"),
        b"```rust\nfn a() {}\n```\n",
    )
    .unwrap();
    let mut app = app_with(&mem);
    load_entries(&mut app, &["a.md"]);
    let mut effects = Effects::default();
    app.explorer.nav.move_by(1, app.explorer.entries.len());
    after_cursor_move(&mut app, &mut effects);
    run_cmds_through_update(&mut app, &mut effects);
    let id = preview_id(&app);
    assert!(
        app.explorer
            .preview
            .as_ref()
            .unwrap()
            .doc
            .highlight
            .in_flight
            == Some(1),
        "test setup: the preview's highlight is in flight when it gets promoted"
    );

    app.set_focus_pane(Pane::Explorer, &mut effects);
    let escape = crate::keymap::KeyInput {
        code: crate::keymap::KeyCode::Escape,
        mods: crate::keymap::Mods::NONE,
    };
    crate::app::update(&mut app, Msg::Key(escape), &mut effects);
    assert_eq!(app.active, id, "test setup: the preview was promoted");

    run_cmds_through_update(&mut app, &mut effects);

    let doc = app.doc(id).expect("the promoted document is open");
    assert!(
        doc.highlight.in_flight.is_none(),
        "the reply must land on the document it named, wherever that document now lives"
    );
    assert!(
        !doc.highlight.regions.is_empty(),
        "a promoted tab keeps the colours its own reply carried"
    );
}
