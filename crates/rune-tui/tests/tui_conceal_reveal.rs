#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod tui_render_common;

use rune_core::coords::{BufferOffset, VisualCol};
use rune_core::cursor::{CursorSet, CursorSpec};
use rune_fuzz::Session;
use rune_tui::render;
use tui_render_common::app_for;

fn rows_for(content: &str, cursor_offset: usize, focused: bool) -> Vec<Vec<render::Cell>> {
    let session = app_for(content, cursor_offset, focused);
    let app = session.app();
    let view = app.active_doc().view.as_ref().expect("synced view");
    render::build_rows(app, app.active_doc(), Some(app.active), view)
}

fn has_cell_at(rows: &[Vec<render::Cell>], offset: usize) -> bool {
    rows.iter()
        .flatten()
        .any(|c| c.buf_offset == Some(offset as u32))
}

fn bg_at(rows: &[Vec<render::Cell>], offset: usize) -> Option<ratatui::style::Color> {
    rows.iter()
        .flatten()
        .find(|c| c.buf_offset == Some(offset as u32))
        .and_then(|c| c.style.bg)
}

fn session_with_selection(content: &str, anchor: usize, position: usize) -> Session {
    let mut session = Session::open("/doc.md", content);
    session.resize(80, 24);
    let spec = CursorSpec {
        position: BufferOffset(position),
        anchor: BufferOffset(anchor),
        desired_col: VisualCol(0),
    };
    session.app_mut().active_doc_mut().cursors = CursorSet::new_from_specs(&[spec]);
    session.app_mut().sync_view();
    session
}

#[test]
fn a_selection_anchored_inside_a_concealed_link_reveals_the_whole_link() {
    let content = "prefix [text](url) suffix";
    let link_open = content.find('[').expect("fixture has a link");
    let inside_url = content.find("url").expect("fixture has a url");
    let outside = content.len() - 1;

    let session = session_with_selection(content, inside_url, outside);
    let app = session.app();
    let view = app.active_doc().view.as_ref().expect("synced view");
    let rows = render::build_rows(app, app.active_doc(), Some(app.active), view);

    assert!(
        has_cell_at(&rows, link_open),
        "the link's own '[' must be on screen: the selection's anchor sits \
         inside the link's url half, so the whole link must reveal even \
         though the caret (the selection's other end) sits outside it \
         entirely"
    );
}

#[test]
fn an_untouched_concealed_link_stays_concealed() {
    let content = "prefix [text](url) suffix";
    let link_open = content.find('[').expect("fixture has a link");
    let outside = content.len() - 1;

    let rows = rows_for(content, outside, true);

    assert!(
        !has_cell_at(&rows, link_open),
        "with nothing touching the link, its delimiters must stay hidden"
    );
}

#[test]
fn bracket_match_on_a_links_own_open_bracket_lights_its_close_bracket() {
    let content = "before [text](url) after";
    let link_open = content.find('[').expect("fixture has a link");
    let text_close = content.find(']').expect("fixture has a link");

    let rows = rows_for(content, link_open, true);
    let session = app_for(content, link_open, true);
    let expected = session.app().theme.chrome.bracket_match_bg.bg;

    assert_eq!(
        bg_at(&rows, text_close),
        expected,
        "the caret sits on the link's own '[': its bracket-match partner \
         ']' must be lit, and it must be on screen because the whole link \
         reveals"
    );
}

#[test]
fn bracket_match_on_a_links_url_close_paren_lights_its_open_paren() {
    let content = "before [text](url) after";
    let url_open = content.find('(').expect("fixture has a link");
    let url_close = content.find(')').expect("fixture has a link");

    let rows = rows_for(content, url_close, true);
    let session = app_for(content, url_close, true);
    let expected = session.app().theme.chrome.bracket_match_bg.bg;

    assert_eq!(
        bg_at(&rows, url_open),
        expected,
        "the caret sits on the link's own url-closing ')': its \
         bracket-match partner '(' must be lit and on screen"
    );
}
