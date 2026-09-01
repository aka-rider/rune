#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

use rune_core::buffer::Buffer;
use rune_core::coords::WrapRow;

use crate::app::App;
use crate::document::Document;
use crate::keymap::{KeyCode, Mods};
use crate::runtime::{CmdKind, Effects, Msg, TimerMsgKey};
use crate::viewport::ScrollMode;

use super::tests::{CTRL, key, pump_index, seeded_app};

fn search(app: &mut App, query: &str, effects: &mut Effects) {
    key(app, KeyCode::Char('F'), CTRL, effects);
    pump_index(app, effects);
    for c in query.chars() {
        key(app, KeyCode::Char(c), Mods::NONE, effects);
    }
    crate::app::update(
        app,
        Msg::Timer {
            key: TimerMsgKey::ProjectSearchDebounce,
            generation: 0,
        },
        effects,
    );
    let reply = run_one_cmd(effects, CmdKind::ProjectQuery).expect("query cmd dispatched");
    crate::app::update(app, reply, effects);
}

pub(super) fn run_one_cmd(effects: &mut Effects, kind: CmdKind) -> Option<Msg> {
    let position = effects.cmds.iter().position(|cmd| cmd.kind() == kind)?;
    effects.cmds.remove(position).run()
}

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

#[test]
fn selecting_a_result_issues_a_preview_request() {
    let mut app = seeded_app(&[("/root/deep.md", long_body().as_slice())]);
    let mut effects = Effects::default();
    search(&mut app, "needle", &mut effects);

    key(&mut app, KeyCode::Down, Mods::NONE, &mut effects);

    assert_eq!(
        app.explorer.preview_awaiting.as_deref(),
        Some(Path::new("/root/deep.md")),
        "selection change must request a preview of the selected hit"
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
    search(&mut app, "needle", &mut effects);
    let first_match = app
        .projectsearch()
        .expect("panel open")
        .results
        .first()
        .expect("a hit")
        .first_match;

    key(&mut app, KeyCode::Down, Mods::NONE, &mut effects);
    let reply = run_one_cmd(&mut effects, CmdKind::ReadFile).expect("preview cmd dispatched");
    crate::app::update(&mut app, reply, &mut effects);

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
        app.projectsearch()
            .expect("panel open")
            .pending_center
            .is_none(),
        "the consumed reply cleared the pending center"
    );
}

#[test]
fn selecting_an_already_open_file_centers_the_real_doc_without_moving_its_cursor() {
    let mut app = seeded_app(&[("/root/deep.md", long_body().as_slice())]);
    let mut effects = Effects::default();
    let id =
        crate::workspace::open_path_checked(&mut app, Path::new("/root/deep.md"), &mut effects)
            .expect("open the file for real");
    let cursor_before = app.doc(id).unwrap().cursors.primary().position.get();
    search(&mut app, "needle", &mut effects);
    let first_match = app
        .projectsearch()
        .expect("panel open")
        .results
        .first()
        .expect("a hit")
        .first_match;

    key(&mut app, KeyCode::Down, Mods::NONE, &mut effects);

    assert!(
        app.explorer.preview.is_none(),
        "an open file never grows a preview doc"
    );
    let doc = app.doc_mut(id).expect("doc lives");
    let h = doc.viewport.height as usize;
    assert!(h > 0);
    let expected_row = expected_display_row(doc, first_match);
    assert_eq!(
        doc.viewport.scroll_row.0,
        expected_row.saturating_sub(h / 2)
    );
    assert_eq!(doc.viewport.mode, ScrollMode::Independent);
    assert_eq!(
        app.doc(id).unwrap().cursors.primary().position.get(),
        cursor_before,
        "centering must never move the cursor"
    );
}

#[test]
fn enter_on_a_previewed_hit_promotes_it_and_lands_on_the_match() {
    let mut app = seeded_app(&[("/root/deep.md", long_body().as_slice())]);
    let mut effects = Effects::default();
    search(&mut app, "needle", &mut effects);
    let first_match = app
        .projectsearch()
        .expect("panel open")
        .results
        .first()
        .expect("a hit")
        .first_match;
    key(&mut app, KeyCode::Down, Mods::NONE, &mut effects);
    let reply = run_one_cmd(&mut effects, CmdKind::ReadFile).expect("preview cmd dispatched");
    crate::app::update(&mut app, reply, &mut effects);
    let preview = app
        .explorer
        .preview
        .as_ref()
        .expect("test setup: the hit is being previewed")
        .id;
    let tabs_before = app.documents.order().len();

    key(&mut app, KeyCode::Enter, Mods::NONE, &mut effects);

    assert!(app.projectsearch().is_none(), "activation closes the panel");
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
    assert_eq!(
        app.active_doc().cursors.primary().position.get(),
        first_match
    );
    assert_eq!(app.active_doc().viewport.mode, ScrollMode::EnsureVisible);
    assert_eq!(app.focus(), crate::pane::Pane::Editor);
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
    search(&mut app, "needle", &mut effects);
    key(&mut app, KeyCode::Down, Mods::NONE, &mut effects);
    let reply = run_one_cmd(&mut effects, CmdKind::ReadFile).expect("preview cmd dispatched");
    crate::app::update(&mut app, reply, &mut effects);
    assert!(
        app.explorer.preview.is_some(),
        "test setup: a preview costs no tab slot, so it exists even when tabs are full"
    );

    key(&mut app, KeyCode::Enter, Mods::NONE, &mut effects);

    assert!(
        app.projectsearch().is_some(),
        "a refused promotion must leave the panel and its results standing"
    );
    assert_eq!(
        crate::messages::newest_text(&app),
        Some("Tab limit reached — close or unpin a tab")
    );
}

#[test]
fn enter_opens_the_file_with_the_cursor_at_the_match_and_closes_the_panel() {
    let mut app = seeded_app(&[("/root/deep.md", long_body().as_slice())]);
    let mut effects = Effects::default();
    search(&mut app, "needle", &mut effects);
    let first_match = app
        .projectsearch()
        .expect("panel open")
        .results
        .first()
        .expect("a hit")
        .first_match;
    assert!(
        first_match > 0,
        "test setup: the match is not at offset zero"
    );

    key(&mut app, KeyCode::Enter, Mods::NONE, &mut effects);

    assert!(app.projectsearch().is_none(), "activation closes the panel");
    assert_eq!(app.active_doc().path(), Some(Path::new("/root/deep.md")));
    assert_eq!(
        app.active_doc().cursors.primary().position.get(),
        first_match
    );
    assert_eq!(app.active_doc().viewport.mode, ScrollMode::EnsureVisible);
    assert_eq!(app.focus(), crate::pane::Pane::Editor);
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
    search(&mut app, "needle", &mut effects);

    key(&mut app, KeyCode::Enter, Mods::NONE, &mut effects);

    assert!(
        app.projectsearch().is_some(),
        "a refused open must leave the panel and its results standing"
    );
    assert_eq!(
        crate::messages::newest_text(&app),
        Some("Tab limit reached — close or unpin a tab")
    );
}

#[test]
fn reopening_prefills_the_last_query_and_reruns_it_against_the_corpus() {
    let mut app = seeded_app(&[("/root/deep.md", long_body().as_slice())]);
    let mut effects = Effects::default();
    search(&mut app, "needle", &mut effects);
    key(&mut app, KeyCode::Escape, Mods::NONE, &mut effects);
    assert!(app.projectsearch().is_none(), "test setup: panel closed");

    key(&mut app, KeyCode::Char('F'), CTRL, &mut effects);

    let state = app.projectsearch().expect("panel reopened");
    assert_eq!(state.query, "needle", "the last query comes back prefilled");
    assert!(
        effects
            .cmds
            .iter()
            .any(|cmd| cmd.kind() == CmdKind::ProjectIndex),
        "reopening kicks off a rescan of the workspace"
    );
    let immediate = run_one_cmd(&mut effects, CmdKind::ProjectQuery)
        .expect("reopening with a live query dispatches it without a debounce");
    crate::app::update(&mut app, immediate, &mut effects);
    assert_eq!(
        app.projectsearch()
            .expect("panel open")
            .results
            .first()
            .map(|hit| hit.display.clone()),
        Some("deep.md".to_string()),
        "the immediate query answers from the existing corpus before the rescan lands"
    );
    pump_index(&mut app, &mut effects);
    let rerun = run_one_cmd(&mut effects, CmdKind::ProjectQuery)
        .expect("refresh completion reruns the current query");
    crate::app::update(&mut app, rerun, &mut effects);
    assert_eq!(
        app.projectsearch()
            .expect("panel open")
            .results
            .first()
            .map(|hit| hit.display.clone()),
        Some("deep.md".to_string())
    );
}

#[test]
fn the_hit_highlight_paints_on_the_previewed_document() {
    let mut app = seeded_app(&[("/root/deep.md", long_body().as_slice())]);
    let mut effects = Effects::default();
    search(&mut app, "needle", &mut effects);
    let hit_ranges = app
        .projectsearch()
        .expect("panel open")
        .results
        .first()
        .expect("a hit")
        .ranges
        .clone();

    key(&mut app, KeyCode::Down, Mods::NONE, &mut effects);
    let reply = run_one_cmd(&mut effects, CmdKind::ReadFile).expect("preview cmd dispatched");
    crate::app::update(&mut app, reply, &mut effects);
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
    let highlighted: Vec<usize> = rows
        .iter()
        .flatten()
        .filter(|cell| cell.style.bg == app.theme.chrome.search_match_bg.bg)
        .filter_map(|cell| cell.buf_offset.map(|offset| offset as usize))
        .collect();
    assert!(
        !highlighted.is_empty(),
        "the selected hit must be painted on the document showing it"
    );
    assert!(
        highlighted
            .iter()
            .all(|offset| hit_ranges.iter().any(|r| r.contains(offset))),
        "only the hit's own byte ranges may carry the match background"
    );
}
