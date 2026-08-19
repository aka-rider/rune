#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
use std::path::PathBuf;
use std::sync::Arc;

use rune_core::buffer::Buffer as CoreBuffer;
use rune_vfs::{Mem, Vfs};

use super::*;
use super::tests_common::{app_with, load_entries, run_cmds, run_cmds_through_update};
use crate::runtime::Msg;

/// A live in-memory `Db` — mirrors `pane.rs::tests::live_db` — needed to
/// prove `promote` actually enqueues recovery-store hydration (`App::db_ops`
/// gains a `Load` entry for the promoted document) rather than merely
/// flipping `read_only`. Every other test in this module runs with
/// `app.db == None`, where `db_enqueue::load_document` is a documented
/// no-op, so none of them could catch a promote that stopped hydrating.
fn live_db() -> crate::db::Db {
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Mem::new());
    let clock: rune_db::ClockFn = Arc::new(std::time::SystemTime::now);
    let store =
        rune_db::Store::open_in_memory(clock, vfs, Box::new(|_evt| {})).expect("open store");
    let bridge = crate::db::DbBridge::bootstrap();
    crate::db::Db::new(store, bridge, false)
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
    let id = app.explorer.preview.expect("preview minted");
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

/// Finding 1 (HIGH): `Document::new` resets `highlight` and resets the
/// swapped-in buffer to version 1 — and a preview's buffer is never
/// edited, so before `apply_loaded` advanced the version past the reused
/// document's own, every preview after the first sat at version 1 forever.
/// `dispatch::after_update`'s highlight-reschedule check is gated on the
/// buffer version actually changing, so nothing was ever scheduled past
/// the first file arrowed onto — this pins that a highlight is scheduled
/// for EVERY file in the run, not just the first.
#[test]
fn arrowing_across_files_schedules_a_highlight_for_every_preview() {
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

        let id = app.explorer.preview.expect("preview minted");
        assert!(
            app.doc(id).expect("doc").highlight.in_flight.is_some(),
            "file #{n} previewed onto the reused document must schedule its own highlight"
        );
    }
}

/// Finding 1 (HIGH): the in-place swap's stale-reply hazard, constructed
/// directly rather than raced across threads — the danger is a version
/// COLLISION (both buffers independently starting at version 1 under the
/// same reused id), not a timing window, so feeding a direct `Msg::
/// Highlighted` at the version captured just before the swap exercises
/// exactly the check a genuinely late reply would hit. Before `apply_
/// loaded` advanced the version, this reply's `version` would have equalled
/// the new file's live version by coincidence and `dispatch::
/// handle_highlighted` would have installed file A's regions onto file B.
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
    let id = app.explorer.preview.expect("preview minted");
    let stale_version = app.doc(id).expect("doc").buffer.version();

    app.explorer.nav.move_by(1, app.explorer.entries.len()); // "b.md"
    after_cursor_move(&mut app, &mut effects);
    run_cmds_through_update(&mut app, &mut effects);
    assert_eq!(app.explorer.preview, Some(id), "the same id is reused");
    let live_version = app.doc(id).expect("doc").buffer.version();
    assert_ne!(
        stale_version, live_version,
        "the swap must advance the buffer's version"
    );

    let scope = rune_syntax::scope::scope_table()
        .resolve("keyword")
        .expect("known scope");
    let marker_line = 0..4;
    crate::app::update(
        &mut app,
        Msg::Highlighted {
            doc: id,
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

    let doc = app.doc(id).expect("doc");
    assert!(
        doc.highlight.regions.is_empty(),
        "a reply computed for the file this id used to show must never install \
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
    let preview_id = app.explorer.preview.expect("preview minted");

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
    assert!(app.doc(preview_id).is_none());
    assert_eq!(app.active, real, "falls back to the surviving real tab");
    assert_eq!(app.focus(), Pane::Tabs);
}

/// Finding 2 (MEDIUM): with several real tabs open and a NON-FIRST one
/// active, discarding a preview via `^t` (`GlobalCommand::FocusTabs`,
/// routed through `on_focus_changed`'s `Pane::Tabs` arm) must restore the
/// document the user was actually editing before browsing — not
/// `documents.order().first()`, which this test's setup deliberately makes a
/// DIFFERENT document so the old tab-0 fallback would be caught.
#[test]
fn discarding_a_preview_via_ctrl_t_restores_the_document_active_before_previewing() {
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
    let preview_id = app.explorer.preview.expect("preview minted");
    assert_eq!(
        app.active, preview_id,
        "browsing lands on the preview itself"
    );
    assert_ne!(
        app.documents.order().first().copied(),
        Some(third),
        "tab 0 must NOT be the document that was active before previewing, \
         or this test could not tell the fix from the old tab-0 fallback"
    );

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

    assert!(app.doc(preview_id).is_none());
    assert_eq!(
        app.active, third,
        "must restore the document active before previewing, not tab 0"
    );
}

/// Finding 2 (MEDIUM): the same restoration, reached through focus landing
/// on the Title pane (`^r`, `GlobalCommand::FocusTitle`) instead of Tabs —
/// `on_focus_changed`'s `Pane::Title | Pane::Tabs` arm shares one
/// `discard_active` for both, so this pins the other half of that match.
///
/// Drives `on_focus_changed` directly rather than through `^r`
/// (`GlobalCommand::FocusTitle`): `focus_title` itself refuses whenever the
/// ACTIVE document is read-only, and the previewed document (`ReadOnly::
/// Preview`) is always active while browsing — so this exact transition has
/// no reachable keymap route today, only whatever future path (a mouse
/// click on the Title bar) lands focus there directly the way `set_focus_
/// pane` does.
#[test]
fn discarding_a_preview_via_focus_to_title_restores_the_document_active_before_previewing() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(std::path::Path::new("/root/a.md"), b"content")
        .unwrap();
    let mut app = app_with(&mem);
    app.open_document(CoreBuffer::new("second"));
    let third = app.open_document(CoreBuffer::new("third"));
    workspace::switch_to(&mut app, third);
    load_entries(&mut app, &["a.md"]);
    let mut effects = Effects::default();
    app.set_focus_pane(Pane::Explorer, &mut effects);
    app.explorer.nav.move_by(1, app.explorer.entries.len());
    after_cursor_move(&mut app, &mut effects);
    run_cmds(&mut app, &mut effects);
    let preview_id = app.explorer.preview.expect("preview minted");

    on_focus_changed(&mut app, Pane::Explorer, Pane::Title);

    assert!(app.doc(preview_id).is_none());
    assert_eq!(
        app.active, third,
        "must restore the document active before previewing, not tab 0"
    );
}

/// Finding 2 (MEDIUM) control case: selecting a specific tab by digit
/// while previewing (`^N`, `GlobalCommand::TabSwitch`) reaches
/// `discard_if_switching_away` — a DIFFERENT route than `discard_active`,
/// already correct before this fix — and lands on exactly the tab the
/// digit named. Pinned here so the same scenario (several tabs, a
/// non-first one active before browsing) is covered across every discard
/// route the finding calls out, not only the two `discard_active` used.
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
    let preview_id = app.explorer.preview.expect("preview minted");
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

    assert!(app.doc(preview_id).is_none());
    assert_eq!(app.active, third, "selecting the tab by digit lands on it");
}

/// Finding 2 (MEDIUM) fallback: the remembered pre-preview document can
/// itself be closed while browsing is still live (e.g. via the Tabs
/// pane's own `^w`). `discard_active` must then fall back to
/// `workspace::close::neighbor_of`'s adjacent-tab pick — reused rather
/// than a second neighbour picker — instead of leaving `app.active`
/// pointed at a document that no longer exists.
#[test]
fn discarding_a_preview_falls_back_to_the_adjacent_tab_when_the_remembered_document_was_closed() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(std::path::Path::new("/root/a.md"), b"content")
        .unwrap();
    let mut app = app_with(&mem);
    let second = app.open_document(CoreBuffer::new("second"));
    let third = app.open_document(CoreBuffer::new("third"));
    workspace::switch_to(&mut app, third);
    load_entries(&mut app, &["a.md"]);
    let mut effects = Effects::default();
    app.set_focus_pane(crate::pane::Pane::Explorer, &mut effects);
    app.explorer.nav.move_by(1, app.explorer.entries.len());
    after_cursor_move(&mut app, &mut effects);
    run_cmds(&mut app, &mut effects);
    let preview_id = app.explorer.preview.expect("preview minted");
    assert_eq!(app.explorer.browsing_origin, Some(third));

    // `third` — the remembered return-to document — closes while the
    // preview is still live, e.g. via the Tabs pane.
    let _ = workspace::close_now(&mut app, third, &mut effects);
    assert!(app.doc(third).is_none());

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

    assert!(app.doc(preview_id).is_none());
    assert!(app.doc(third).is_none(), "the closed document stays closed");
    assert!(
        app.doc(second).is_some() && app.active == second,
        "falls back to the surviving neighbour, not the stale remembered document"
    );
}
