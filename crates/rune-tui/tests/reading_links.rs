#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_nav::{DefRole, RefKind};
use rune_syntax::element::ByteRange;
use rune_tui::app::{self, App};
use rune_tui::document::ReadOnly;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::runtime::{Effects, Msg};
use rune_tui::testgrid;
use rune_vfs::Mem;

const WIDTH: u16 = 80;
const HEIGHT: u16 = 40;

fn linked_doc() -> String {
    let mut lines = vec!["[alpha](#Section-A)".to_string()];
    for i in 1..=10 {
        lines.push(format!("filler {i}"));
    }
    lines.push("[beta](#Section-B)".to_string());
    lines.push(String::new());
    lines.push("## Section-A".to_string());
    lines.push("body a".to_string());
    lines.push(String::new());
    lines.push("## Section-B".to_string());
    lines.push("body b".to_string());
    lines.join("\n") + "\n"
}

fn app_basic(content: &str) -> App {
    let mut app = App::new(Buffer::new(content), None, Arc::new(Mem::new()), None);
    app.active_doc_mut().viewport.set_size(WIDTH, HEIGHT - 1);
    app.sync_view();
    app
}

fn plain(code: KeyCode) -> Msg {
    Msg::Key(KeyInput {
        code,
        mods: Mods::NONE,
    })
}

fn shifted(code: KeyCode) -> Msg {
    Msg::Key(KeyInput {
        code,
        mods: Mods {
            shift: true,
            ..Mods::NONE
        },
    })
}

fn ctrl(c: char) -> Msg {
    Msg::Key(KeyInput {
        code: KeyCode::Char(c),
        mods: Mods {
            ctrl: true,
            ..Mods::NONE
        },
    })
}

fn send(app: &mut App, msg: Msg) {
    let mut effects = Effects::default();
    app::update(app, msg, &mut effects);
}

fn enter_reading(app: &mut App) {
    send(app, ctrl('p'));
    assert_eq!(app.active_doc().read_only, ReadOnly::Reading);
}

fn alpha_site(content: &str) -> ByteRange {
    let start = content.find("[alpha]").expect("alpha link in fixture");
    let end = start + content[start..].find(')').expect("closing paren") + 1;
    ByteRange::new(start, end)
}

fn beta_site(content: &str) -> ByteRange {
    let start = content.find("[beta]").expect("beta link in fixture");
    let end = start + content[start..].find(')').expect("closing paren") + 1;
    ByteRange::new(start, end)
}

fn heading_offset(app: &App, name: &str) -> usize {
    app.active_doc()
        .catalogue
        .iter()
        .find_map(|r| match &r.kind {
            RefKind::Def {
                role: DefRole::Heading(_),
                name: def_name,
            } if def_name == name => Some(r.site.start),
            _ => None,
        })
        .expect("heading def with this name exists")
}

#[test]
fn tab_focuses_the_first_link_at_or_after_the_caret() {
    let content = linked_doc();
    let mut app = app_basic(&content);
    enter_reading(&mut app);

    for _ in 0..5 {
        send(&mut app, plain(KeyCode::Down));
    }

    send(&mut app, plain(KeyCode::Tab));

    assert_eq!(
        app.active_doc().reading_link_focus,
        Some(beta_site(&content))
    );
    assert_eq!(
        app.active_doc().cursors.primary().position.get(),
        beta_site(&content).start
    );
}

#[test]
fn shift_tab_steps_back_to_the_previous_link() {
    let content = linked_doc();
    let mut app = app_basic(&content);
    enter_reading(&mut app);

    send(&mut app, plain(KeyCode::Tab));
    send(&mut app, plain(KeyCode::Tab));
    assert_eq!(
        app.active_doc().reading_link_focus,
        Some(beta_site(&content))
    );

    send(&mut app, shifted(KeyCode::Tab));
    assert_eq!(
        app.active_doc().reading_link_focus,
        Some(alpha_site(&content))
    );
}

#[test]
fn tab_wraps_from_the_last_link_to_the_first() {
    let content = linked_doc();
    let mut app = app_basic(&content);
    enter_reading(&mut app);

    send(&mut app, plain(KeyCode::Tab));
    send(&mut app, plain(KeyCode::Tab));
    assert_eq!(
        app.active_doc().reading_link_focus,
        Some(beta_site(&content))
    );

    send(&mut app, plain(KeyCode::Tab));
    assert_eq!(
        app.active_doc().reading_link_focus,
        Some(alpha_site(&content))
    );
}

#[test]
fn shift_tab_wraps_from_the_first_link_to_the_last() {
    let content = linked_doc();
    let mut app = app_basic(&content);
    enter_reading(&mut app);

    send(&mut app, plain(KeyCode::Tab));
    assert_eq!(
        app.active_doc().reading_link_focus,
        Some(alpha_site(&content))
    );

    send(&mut app, shifted(KeyCode::Tab));
    assert_eq!(
        app.active_doc().reading_link_focus,
        Some(beta_site(&content))
    );
}

fn find_word_cells(buf: &ratatui::buffer::Buffer, needle: &str, w: u16, h: u16) -> Vec<(u16, u16)> {
    let chars: Vec<String> = needle.chars().map(|c| c.to_string()).collect();
    for y in 0..h {
        for x0 in 0..w {
            let matched = chars.iter().enumerate().all(|(k, ch)| {
                let x = x0 + u16::try_from(k).unwrap_or(u16::MAX);
                buf.cell((x, y))
                    .is_some_and(|cell| cell.symbol() == ch.as_str())
            });
            if matched {
                return chars
                    .iter()
                    .enumerate()
                    .map(|(k, _)| (x0 + u16::try_from(k).unwrap_or(u16::MAX), y))
                    .collect();
            }
        }
    }
    Vec::new()
}

#[test]
fn focused_link_renders_reversed_while_its_neighbour_does_not() {
    let content = linked_doc();
    let mut app = app_basic(&content);
    enter_reading(&mut app);
    send(&mut app, plain(KeyCode::Tab));

    let buf = testgrid::draw(&app, WIDTH, HEIGHT);

    let alpha_cells = find_word_cells(&buf, "alpha", WIDTH, HEIGHT);
    assert!(
        !alpha_cells.is_empty(),
        "expected to find the focused link's text on screen"
    );
    for (x, y) in &alpha_cells {
        let cell = buf.cell((*x, *y)).expect("just matched");
        assert!(
            cell.modifier.contains(ratatui::style::Modifier::REVERSED),
            "the focused link must render reversed"
        );
    }

    let beta_cells = find_word_cells(&buf, "beta", WIDTH, HEIGHT);
    assert!(
        !beta_cells.is_empty(),
        "expected to find the neighbour link's text on screen"
    );
    for (x, y) in &beta_cells {
        let cell = buf.cell((*x, *y)).expect("just matched");
        assert!(
            !cell.modifier.contains(ratatui::style::Modifier::REVERSED),
            "the unfocused neighbour link must not render reversed"
        );
    }
}

#[test]
fn enter_follows_the_focused_link() {
    let content = linked_doc();
    let mut app = app_basic(&content);
    enter_reading(&mut app);
    send(&mut app, plain(KeyCode::Tab));

    let expected = heading_offset(&app, "Section-A");
    send(&mut app, plain(KeyCode::Enter));

    assert_eq!(app.active_doc().cursors.primary().position.get(), expected);
}

#[test]
fn tab_in_an_editable_document_still_indents_and_does_not_move_link_focus() {
    let content = "[a](http://example.com)\nworld\n";
    let mut app = app_basic(content);
    assert_eq!(app.active_doc().read_only, ReadOnly::No);

    send(&mut app, plain(KeyCode::Tab));

    assert_eq!(
        app.active_doc().buffer.content(),
        "\t[a](http://example.com)\nworld\n"
    );
    assert_eq!(app.active_doc().reading_link_focus, None);
}
