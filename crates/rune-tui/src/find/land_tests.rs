use std::path::{Path, PathBuf};

use rune_core::buffer::Buffer;
use rune_core::coords::DisplayRow;
use rune_core::cursor::CursorSet;

use super::test_support::*;
use crate::app::App;
use crate::document::DocumentId;
use crate::keymap::KeyCode;
use crate::runtime::{CmdKind, Effects};
use crate::viewport::ScrollMode;

const LIMIT_MESSAGE: &str = "Tab limit reached \u{2014} close or unpin a tab";

fn deep_body() -> Vec<u8> {
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

fn long_origin() -> Vec<u8> {
    (0..200)
        .map(|i| format!("line {i}\n"))
        .collect::<String>()
        .into_bytes()
}

fn workspace_find(app: &mut App) -> Effects {
    let mut effects = open_project_find(app);
    pump_index(app, &mut effects);
    effects
}

fn query_and_reply(app: &mut App, text: &str, effects: &mut Effects) {
    type_into(app, text, effects);
    fire_debounce(app, effects);
    deliver_query(app, effects);
}

fn open_real(app: &mut App, path: &str, effects: &mut Effects) -> DocumentId {
    crate::workspace::open_path_checked(app, Path::new(path), effects).expect("the file opens")
}

fn previewed(app: &App) -> Option<PathBuf> {
    crate::explorer_preview::shown_path(app).map(Path::to_path_buf)
}

fn read_requested(effects: &Effects) -> bool {
    effects
        .cmds
        .iter()
        .any(|cmd| cmd.kind() == CmdKind::ReadFile)
}

fn place_origin(app: &mut App, offset: usize, scroll: usize) {
    let doc = app.active_doc_mut();
    doc.set_cursors(CursorSet::new(offset));
    doc.viewport.scroll_row = DisplayRow(scroll);
    doc.viewport.mode = ScrollMode::Independent;
    app.sync_view();
}

fn click_into_editor(app: &mut App) {
    let editor = crate::layout::geometry(app.frame_area(), app).editor;
    click(app, editor.x + 3, editor.y);
}

#[test]
fn a_hit_only_in_an_unopened_file_previews_it_without_adding_a_tab() {
    let mut app = seeded_app(&[("/root/deep.md", deep_body().as_slice())]);
    let draft = app.active;
    let tabs = app.documents.order().len();
    let mut effects = workspace_find(&mut app);

    query_and_reply(&mut app, "needle", &mut effects);
    deliver_preview(&mut app, &mut effects);

    assert_eq!(previewed(&app), Some(PathBuf::from("/root/deep.md")));
    assert_eq!(app.active, draft, "the preview is not a tab");
    assert_eq!(app.documents.order().len(), tabs);
    let preview = app.explorer.preview.as_ref().expect("a preview");
    assert!(
        preview.doc.viewport.scroll_row.0 > 0,
        "the match, deep in the file, is scrolled into view"
    );
}

#[test]
fn a_hit_in_the_active_document_wins_over_a_file_with_more_hits() {
    let mut app = seeded_app(&[
        ("/root/a.md", b"x needle y".as_slice()),
        ("/root/big.md", b"needle needle needle".as_slice()),
    ]);
    let mut effects = Effects::default();
    let a = open_real(&mut app, "/root/a.md", &mut effects);
    place_origin(&mut app, 0, 0);
    let mut effects = workspace_find(&mut app);

    query_and_reply(&mut app, "needle", &mut effects);

    assert_eq!(app.active, a);
    assert_eq!(selection_start(&app), 2, "the hit is selected in place");
    assert!(app.active_doc().cursors().primary().has_selection());
    assert!(app.explorer.preview.is_none());
    assert!(!read_requested(&effects), "no preview was requested");
    let hits = &project(&app).results;
    assert_eq!(hits.len(), 2);
    assert_eq!(
        hits.get(project(&app).list.cursor)
            .map(|hit| hit.path.clone()),
        Some(PathBuf::from("/root/a.md")),
        "the list selection follows the hit the user sees"
    );
}

#[test]
fn enter_keeps_the_hit_the_user_sees_instead_of_the_files_first_match() {
    let mut app = seeded_app(&[("/root/a.md", b"needle ... needle".as_slice())]);
    let mut effects = Effects::default();
    open_real(&mut app, "/root/a.md", &mut effects);
    place_origin(&mut app, 3, 0);
    let mut effects = workspace_find(&mut app);
    query_and_reply(&mut app, "needle", &mut effects);
    assert_eq!(selection_start(&app), 11, "the first hit after the cursor");

    press_into(&mut app, enter(), &mut effects);

    assert_eq!(selection_start(&app), 11);
}

#[test]
fn an_active_document_hit_before_the_cursor_yields_to_the_first_file() {
    let mut app = seeded_app(&[
        ("/root/a.md", b"needle".as_slice()),
        ("/root/big.md", b"needle needle".as_slice()),
    ]);
    let mut effects = Effects::default();
    let a = open_real(&mut app, "/root/a.md", &mut effects);
    place_origin(&mut app, 6, 0);
    let mut effects = workspace_find(&mut app);

    query_and_reply(&mut app, "needle", &mut effects);
    deliver_preview(&mut app, &mut effects);

    assert_eq!(app.active, a);
    assert_eq!(previewed(&app), Some(PathBuf::from("/root/big.md")));
    assert_eq!(selection_start(&app), 6, "the origin cursor was restored");
    assert!(!app.active_doc().cursors().primary().has_selection());
}

#[test]
fn a_late_reply_does_not_pull_the_view_off_a_hit_already_landed() {
    let mut app = seeded_app(&[
        ("/root/a.md", b"xxx need".as_slice()),
        ("/root/big.md", b"nee nee nee".as_slice()),
    ]);
    let mut effects = Effects::default();
    open_real(&mut app, "/root/a.md", &mut effects);
    place_origin(&mut app, 0, 0);
    let mut effects = workspace_find(&mut app);
    type_into(&mut app, "nee", &mut effects);
    fire_debounce(&mut app, &mut effects);
    type_into(&mut app, "d", &mut effects);
    assert_eq!(selection_start(&app), 4);

    deliver_query(&mut app, &mut effects);

    assert!(app.explorer.preview.is_none());
    assert!(app.explorer.preview_awaiting.is_none());
    assert!(!read_requested(&effects));
    assert_eq!(selection_start(&app), 4);
}

#[test]
fn a_reply_arriving_after_the_user_clicked_into_the_editor_navigates_nowhere() {
    let mut app = seeded_app(&[("/root/deep.md", deep_body().as_slice())]);
    let draft = app.active;
    let mut effects = workspace_find(&mut app);
    type_into(&mut app, "needle", &mut effects);
    fire_debounce(&mut app, &mut effects);
    click_into_editor(&mut app);
    assert!(!find(&app).focused, "test setup: the click left the panel");

    deliver_query(&mut app, &mut effects);

    assert!(app.explorer.preview.is_none());
    assert!(!read_requested(&effects));
    assert_eq!(app.active, draft);
    assert_eq!(project(&app).results.len(), 1, "the list still updates");
}

#[test]
fn escape_discards_the_preview_and_restores_the_origin_document_cursor_and_scroll() {
    let mut app = seeded_app(&[
        ("/root/origin.md", long_origin().as_slice()),
        ("/root/deep.md", deep_body().as_slice()),
    ]);
    let mut effects = Effects::default();
    let origin = open_real(&mut app, "/root/origin.md", &mut effects);
    place_origin(&mut app, 100, 30);
    let cursors = app.active_doc().cursors().clone();
    let scroll = app.active_doc().viewport.scroll_row;
    assert!(scroll.0 > 0, "test setup: the origin is scrolled");
    let mut effects = workspace_find(&mut app);
    query_and_reply(&mut app, "needle", &mut effects);
    deliver_preview(&mut app, &mut effects);
    assert!(app.explorer.preview.is_some(), "test setup: previewing");

    press_into(&mut app, esc_key(), &mut effects);
    app.sync_view();

    assert!(app.find().is_none());
    assert!(app.explorer.preview.is_none());
    assert!(app.explorer.preview_awaiting.is_none());
    assert_eq!(app.active, origin);
    assert_eq!(*app.active_doc().cursors(), cursors);
    assert_eq!(app.active_doc().viewport.scroll_row, scroll);
}

#[test]
fn escape_from_a_hit_shown_in_another_open_tab_returns_to_the_origin_cursor_and_scroll() {
    let mut app = seeded_app(&[
        ("/root/origin.md", long_origin().as_slice()),
        ("/root/deep.md", deep_body().as_slice()),
    ]);
    let mut effects = Effects::default();
    open_real(&mut app, "/root/deep.md", &mut effects);
    let origin = open_real(&mut app, "/root/origin.md", &mut effects);
    place_origin(&mut app, 100, 30);
    let cursors = app.active_doc().cursors().clone();
    let scroll = app.active_doc().viewport.scroll_row;
    let mut effects = workspace_find(&mut app);
    query_and_reply(&mut app, "needle", &mut effects);
    assert_ne!(app.active, origin, "test setup: the open tab is shown");

    press_into(&mut app, esc_key(), &mut effects);
    app.sync_view();

    assert_eq!(app.active, origin);
    assert_eq!(*app.active_doc().cursors(), cursors);
    assert_eq!(app.active_doc().viewport.scroll_row, scroll);
}

#[test]
fn enter_promotes_the_preview_and_the_tab_limit_is_reported_when_nothing_can_make_room() {
    let mut app = seeded_app(&[("/root/deep.md", deep_body().as_slice())]);
    for i in 1..crate::opentabs::limit::MAX_TABS {
        let path = crate::resolved::ResolvedPath::resolve(
            app.vfs.as_ref(),
            Path::new(&format!("/root/f{i}.md")),
        )
        .expect("Mem resolves any spelling");
        let id = app.open_document_bound(Buffer::new("x"), path);
        if i % 2 == 0 {
            app.doc_mut(id).expect("opened").pinned = true;
        } else {
            crate::commands::edit::insert_char(&mut app, id, '!');
        }
    }
    let draft = app.active;
    app.doc_mut(draft).expect("draft").pinned = true;
    let tabs = app.documents.order().len();
    assert_eq!(tabs, crate::opentabs::limit::MAX_TABS);
    let mut effects = workspace_find(&mut app);
    query_and_reply(&mut app, "needle", &mut effects);
    deliver_preview(&mut app, &mut effects);

    press_into(&mut app, enter(), &mut effects);

    assert_eq!(
        crate::messages::newest_text(&app),
        Some(LIMIT_MESSAGE),
        "a refused promotion says why"
    );
    assert_eq!(previewed(&app), Some(PathBuf::from("/root/deep.md")));
    assert_eq!(app.documents.order().len(), tabs);
    assert!(app.find().is_some());
}

#[test]
fn the_preview_rejects_edits_and_keystrokes_go_to_the_panel() {
    let mut app = seeded_app(&[("/root/deep.md", deep_body().as_slice())]);
    let mut effects = workspace_find(&mut app);
    query_and_reply(&mut app, "needle", &mut effects);
    deliver_preview(&mut app, &mut effects);
    let preview = app.explorer.preview.as_ref().expect("a preview");
    let id = preview.id;
    let before = preview.doc.buffer.content().to_string();

    crate::commands::edit::insert_char(&mut app, id, 'X');
    press_into(&mut app, char_key('Y'), &mut effects);

    let preview = app.explorer.preview.as_ref().expect("still previewing");
    assert_eq!(preview.doc.buffer.content(), before);
    assert!(!preview.doc.is_dirty());
    assert_eq!(find(&app).find.editor.text(), "needleY");
}

#[test]
fn typing_into_a_previewed_file_promotes_it_so_escape_cannot_drop_the_words() {
    let mut app = seeded_app(&[("/root/deep.md", deep_body().as_slice())]);
    let mut effects = workspace_find(&mut app);
    query_and_reply(&mut app, "needle", &mut effects);
    deliver_preview(&mut app, &mut effects);
    let id = app.explorer.preview.as_ref().expect("a preview").id;

    click_into_editor(&mut app);
    press_into(&mut app, char_key('Z'), &mut effects);
    press_into(&mut app, esc_key(), &mut effects);

    assert!(app.find().is_none());
    assert_eq!(app.active, id, "the promoted file stays active");
    assert!(app.doc(id).expect("a real tab").is_dirty());
    assert!(
        app.doc(id)
            .expect("a real tab")
            .buffer
            .content()
            .contains('Z')
    );
}

#[test]
fn the_tabs_pane_lists_the_preview_in_italics_and_a_click_on_it_promotes_it() {
    let mut app = seeded_app(&[("/root/deep.md", deep_body().as_slice())]);
    app.splits.left.show();
    let mut effects = workspace_find(&mut app);
    query_and_reply(&mut app, "needle", &mut effects);
    deliver_preview(&mut app, &mut effects);
    app.sync_view();
    let tabs = crate::layout::geometry(app.frame_area(), &app).tabs_inner;
    let buf = crate::testgrid::draw(&app, PROJECT_W, PROJECT_H);
    let row = (tabs.y..tabs.y + tabs.height)
        .find(|&y| {
            (0..PROJECT_W)
                .filter_map(|x| buf.cell((x, y)).map(|c| c.symbol().to_string()))
                .collect::<String>()
                .contains("deep.md")
        })
        .expect("the preview has a row in the tabs pane");
    let cell = buf.cell((tabs.x + 8, row)).expect("a cell");
    assert!(
        cell.style()
            .add_modifier
            .contains(ratatui::style::Modifier::ITALIC)
    );
    let id = app.explorer.preview.as_ref().expect("a preview").id;

    click(&mut app, tabs.x + 9, row);

    assert_eq!(app.active, id);
    assert!(app.explorer.preview.is_none());
}

#[test]
fn leaving_the_workspace_filter_returns_to_the_origin_document() {
    let mut app = seeded_app(&[
        ("/root/deep.md", deep_body().as_slice()),
        ("/root/origin.md", long_origin().as_slice()),
    ]);
    let mut effects = Effects::default();
    open_real(&mut app, "/root/deep.md", &mut effects);
    let origin = open_real(&mut app, "/root/origin.md", &mut effects);
    let mut effects = workspace_find(&mut app);
    query_and_reply(&mut app, "needle", &mut effects);
    assert_ne!(app.active, origin, "test setup: the hit's tab is shown");

    press_into(&mut app, key(KeyCode::Char('f'), ALT), &mut effects);

    assert_eq!(app.active, origin);
}

#[test]
fn a_query_with_no_hit_anywhere_restores_the_origin_and_drops_the_preview() {
    let mut app = seeded_app(&[("/root/deep.md", deep_body().as_slice())]);
    place_origin(&mut app, 2, 0);
    let mut effects = workspace_find(&mut app);
    query_and_reply(&mut app, "needle", &mut effects);
    deliver_preview(&mut app, &mut effects);
    assert!(app.explorer.preview.is_some(), "test setup: previewing");

    query_and_reply(&mut app, "x", &mut effects);

    assert!(app.explorer.preview.is_none());
    assert_eq!(selection_start(&app), 2);
    assert!(project(&app).results.is_empty());
}

#[test]
fn an_empty_query_follows_nothing_and_queries_nothing() {
    let mut app = seeded_app(&[("/root/deep.md", deep_body().as_slice())]);
    let mut effects = workspace_find(&mut app);

    fire_debounce(&mut app, &mut effects);

    assert!(app.explorer.preview.is_none());
    assert!(
        !effects
            .cmds
            .iter()
            .any(|cmd| cmd.kind() == CmdKind::ProjectQuery)
    );
    assert_eq!(selection_start(&app), 0);
}

#[test]
fn a_one_char_query_follows_only_in_the_active_document() {
    let mut app = seeded_app(&[("/root/deep.md", deep_body().as_slice())]);
    place_origin(&mut app, 1, 0);
    let mut effects = workspace_find(&mut app);

    type_into(&mut app, "l", &mut effects);
    fire_debounce(&mut app, &mut effects);

    assert_eq!(selection_start(&app), 2, "the draft's own `l` is selected");
    assert!(app.explorer.preview.is_none());
    assert!(
        !effects
            .cmds
            .iter()
            .any(|cmd| cmd.kind() == CmdKind::ProjectQuery),
        "one character is below the project minimum"
    );
}

#[test]
fn a_one_char_query_with_no_hit_in_the_active_document_leaves_the_origin_alone() {
    let mut app = seeded_app(&[("/root/deep.md", deep_body().as_slice())]);
    place_origin(&mut app, 2, 0);
    let mut effects = workspace_find(&mut app);

    type_into(&mut app, "z", &mut effects);
    fire_debounce(&mut app, &mut effects);

    assert_eq!(selection_start(&app), 2);
    assert!(!app.active_doc().cursors().primary().has_selection());
    assert!(app.explorer.preview.is_none());
}
