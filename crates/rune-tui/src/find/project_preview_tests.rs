use std::path::Path;
use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_core::coords::WrapRow;
use rune_vfs::Mem;

use super::test_support::*;
use crate::app::App;
use crate::document::Document;
use crate::runtime::{CmdKind, Effects};
use crate::viewport::ScrollMode;

fn expected_display_row(doc: &mut Document, offset: usize) -> usize {
    let view = doc.view();
    let bp = doc.buffer.offset_to_line_col(offset);
    let sp = view.syntax.buffer_to_syntax(bp);
    let wrap_row = WrapRow(view.wrap.syntax_to_wrap(sp).row);
    view.display.wrap_to_display(wrap_row).0
}

fn long_body() -> Vec<u8> {
    let mut body = String::new();
    for i in 0..100 {
        if i == 60 {
            body.push_str("needle here\n");
        } else {
            body.push_str(&format!("line {i:03}\n"));
        }
        body.push('\n');
    }
    body.into_bytes()
}

fn first_match(app: &App) -> usize {
    project(app).results.first().expect("a hit").first_match
}

fn deliver_preview(app: &mut App, effects: &mut Effects) {
    let reply = run_one_cmd(effects, CmdKind::ReadFile).expect("preview cmd dispatched");
    crate::app::update(app, reply, effects);
}

fn in_memory_db() -> crate::db::Db {
    crate::db::Db::new(
        rune_db::Store::open_in_memory(
            Arc::new(std::time::SystemTime::now),
            Arc::new(Mem::new()),
            Box::new(|_evt| {}),
        )
        .expect("open in-memory store"),
        crate::db::DbBridge::bootstrap(),
        false,
    )
}

#[test]
fn results_arriving_for_an_unopened_file_issue_a_preview_request() {
    let mut app = seeded_app(&[("/root/deep.md", long_body().as_slice())]);
    let mut effects = Effects::default();

    search_project(&mut app, "needle", &mut effects);

    assert_eq!(
        app.explorer.preview_awaiting.as_deref(),
        Some(Path::new("/root/deep.md")),
        "the top hit is previewed as soon as results land"
    );
    assert!(
        effects
            .cmds
            .iter()
            .any(|cmd| cmd.kind() == CmdKind::ReadFile),
        "the preview read left the thread as a cmd"
    );
}

#[test]
fn the_consumed_preview_reply_centers_the_first_match() {
    let mut app = seeded_app(&[("/root/deep.md", long_body().as_slice())]);
    let mut effects = Effects::default();
    search_project(&mut app, "needle", &mut effects);
    let first_match = first_match(&app);

    deliver_preview(&mut app, &mut effects);

    let target = crate::workspace::shown_document_for(&app, Path::new("/root/deep.md"))
        .expect("the reply created a preview");
    let doc = app.live_doc_mut(target).expect("preview doc lives");
    let h = doc.viewport.height as usize;
    assert!(h > 0, "a zero-height viewport would make centering vacuous");
    let expected_row = expected_display_row(doc, first_match);
    assert!(
        expected_row > h,
        "test setup: the match sits below one page"
    );
    assert_eq!(
        doc.viewport.scroll_row.0,
        expected_row.saturating_sub(h / 2)
    );
    assert_eq!(doc.viewport.mode, ScrollMode::Independent);
    assert!(
        project(&app).pending_center.is_none(),
        "the consumed reply cleared the pending center"
    );
}

#[test]
fn a_hit_in_the_active_document_selects_its_first_match_on_arrival() {
    let mut app = seeded_app(&[("/root/deep.md", long_body().as_slice())]);
    let mut effects = Effects::default();
    let id =
        crate::workspace::open_path_checked(&mut app, Path::new("/root/deep.md"), &mut effects)
            .expect("open the file for real");
    assert_eq!(
        app.active, id,
        "test setup: the hit file is the active document"
    );

    search_project(&mut app, "needle", &mut effects);
    let first_match = first_match(&app);

    assert!(
        app.explorer.preview.is_none(),
        "an open file never grows a preview doc"
    );
    assert_eq!(selection_start(&app), first_match);
    assert_eq!(find(&app).current, Some(0));
    assert!(
        app.active_doc().cursors.primary().has_selection(),
        "the first match is selected like a File-scope follow"
    );
}

#[test]
fn a_hit_in_an_open_but_inactive_document_switches_to_it_and_centers_without_moving_its_cursor() {
    let mut app = seeded_app(&[("/root/deep.md", long_body().as_slice())]);
    let mut effects = Effects::default();
    let hit_doc =
        crate::workspace::open_path_checked(&mut app, Path::new("/root/deep.md"), &mut effects)
            .expect("open the file for real");
    let cursor_before = app.doc(hit_doc).unwrap().cursors.primary().position.get();
    let origin = app.open_document(Buffer::new("origin"));
    crate::workspace::switch_to(&mut app, origin);

    search_project(&mut app, "needle", &mut effects);
    let first_match = first_match(&app);

    assert_eq!(app.active, hit_doc, "the top hit's open tab is shown");
    let doc = app.doc_mut(hit_doc).expect("doc lives");
    let h = doc.viewport.height as usize;
    assert!(h > 0);
    let expected_row = expected_display_row(doc, first_match);
    assert_eq!(
        doc.viewport.scroll_row.0,
        expected_row.saturating_sub(h / 2)
    );
    assert_eq!(
        app.doc(hit_doc).unwrap().cursors.primary().position.get(),
        cursor_before,
        "centering another tab must never move its cursor"
    );

    press_into(&mut app, escape(), &mut effects);
    assert!(app.find().is_none());
    assert_eq!(
        app.active, origin,
        "escape before any commit returns to the origin document"
    );
}

#[test]
fn enter_on_a_previewed_hit_promotes_it_lands_on_the_match_and_keeps_the_panel() {
    let mut app = seeded_app(&[("/root/deep.md", long_body().as_slice())]);
    let mut effects = Effects::default();
    search_project(&mut app, "needle", &mut effects);
    let first_match = first_match(&app);
    deliver_preview(&mut app, &mut effects);
    let preview = app
        .explorer
        .preview
        .as_ref()
        .expect("test setup: the hit is being previewed")
        .id;
    let tabs_before = app.documents.order().len();

    press_into(&mut app, enter(), &mut effects);

    assert!(find(&app).focused, "activation keeps the panel focused");
    assert!(
        app.explorer.preview.is_none(),
        "the promoted preview leaves the Explorer's slot"
    );
    assert_eq!(app.active, preview, "the previewed document becomes active");
    assert_eq!(
        app.documents.order().len(),
        tabs_before + 1,
        "promotion claims the tab the preview never held"
    );
    assert_eq!(selection_start(&app), first_match);
    assert_eq!(
        find(&app).origin.doc,
        preview,
        "the opened hit becomes the origin"
    );
    assert_eq!(app.active_doc().viewport.mode, ScrollMode::EnsureVisible);
    assert_eq!(
        app.nav_history.len(),
        1,
        "promotion is the one place that records the departure"
    );
}

#[test]
fn enter_on_a_previewed_hit_under_a_full_tab_limit_leaves_the_panel_open() {
    let mut app = seeded_app(&[("/root/deep.md", long_body().as_slice())]);
    for _ in 1..crate::opentabs::limit::MAX_TABS {
        app.open_document(Buffer::new("draft"));
    }
    let mut effects = Effects::default();
    search_project(&mut app, "needle", &mut effects);
    deliver_preview(&mut app, &mut effects);
    assert!(
        app.explorer.preview.is_some(),
        "test setup: a preview costs no tab slot, so it exists even when tabs are full"
    );

    press_into(&mut app, enter(), &mut effects);

    assert!(
        app.find().is_some(),
        "a refused promotion must leave the panel and its results standing"
    );
    assert_eq!(
        crate::messages::newest_text(&app),
        Some("Tab limit reached — close or unpin a tab")
    );
}

#[test]
fn enter_in_the_find_field_opens_the_top_hit_and_the_panel_stays_open_and_focused() {
    let mut app = seeded_app(&[("/root/deep.md", long_body().as_slice())]);
    app.db = Some(in_memory_db());
    let mut effects = Effects::default();
    search_project(&mut app, "needle", &mut effects);
    let first_match = first_match(&app);
    assert!(
        first_match > 0,
        "test setup: the match is not at offset zero"
    );

    press_into(&mut app, enter(), &mut effects);

    assert!(find(&app).focused, "activation keeps the panel focused");
    assert_eq!(find(&app).focus, crate::find::Control::Find);
    assert_eq!(app.active_doc().path(), Some(Path::new("/root/deep.md")));
    assert_eq!(selection_start(&app), first_match);
    assert_eq!(app.active_doc().viewport.mode, ScrollMode::EnsureVisible);
    assert_eq!(
        app.search_history.ops.len(),
        1,
        "opening a hit commits the query to the shared find history"
    );
    assert_eq!(
        app.last_find,
        Some((
            "needle".to_string(),
            crate::find::matcher::MatchOptions::default()
        ))
    );
}

#[test]
fn enter_under_a_full_tab_limit_leaves_the_panel_open() {
    let mut app = seeded_app(&[("/root/deep.md", long_body().as_slice())]);
    for _ in 1..crate::opentabs::limit::MAX_TABS {
        app.open_document(Buffer::new("draft"));
    }
    assert_eq!(
        app.documents.order().len(),
        crate::opentabs::limit::MAX_TABS,
        "test setup: every tab slot is a draft no eviction may claim"
    );
    let mut effects = Effects::default();
    search_project(&mut app, "needle", &mut effects);
    crate::explorer_preview::discard(&mut app);

    press_into(&mut app, enter(), &mut effects);

    assert!(
        app.find().is_some(),
        "a refused open must leave the panel and its results standing"
    );
    assert_eq!(
        crate::messages::newest_text(&app),
        Some("Tab limit reached — close or unpin a tab")
    );
}

#[test]
fn the_hit_highlight_paints_on_the_previewed_document_with_the_first_match_stronger() {
    let mut app = seeded_app(&[("/root/deep.md", long_body().as_slice())]);
    let mut effects = Effects::default();
    search_project(&mut app, "needle", &mut effects);
    let hit_ranges = project(&app).results.first().expect("a hit").ranges.clone();

    deliver_preview(&mut app, &mut effects);
    app.sync_view();

    assert!(
        app.showing_preview(),
        "test setup: the hit is being previewed, not opened"
    );
    let view = app
        .shown_doc()
        .view
        .as_ref()
        .expect("the preview is laid out");
    let rows = crate::render::build_rows(&app, crate::render::RowSource::Shown, view);
    let current: Vec<usize> = rows
        .iter()
        .flatten()
        .filter(|cell| cell.style.bg == app.theme.chrome.search_current_bg.bg)
        .filter_map(|cell| cell.buf_offset.map(|offset| offset as usize))
        .collect();
    assert!(
        !current.is_empty(),
        "the selected hit's first match must be painted on the document showing it"
    );
    assert!(
        current
            .iter()
            .all(|offset| hit_ranges.first().is_some_and(|r| r.contains(offset))),
        "only the first match carries the current-match background"
    );
}
