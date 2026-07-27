//! WP3 "Done when" tests: `Msg::Error` opens the modal banner, stage 1 of
//! `handle_key` consumes every key while it's up (`Esc` clears it, `c`/`C`
//! copies via OSC 52 then clears it, an unbound key/quit chord is a
//! consumed no-op that leaves the modal untouched, and — crucially — never
//! reaches the editor's own buffer), the banner renders above the footer
//! with its height capped at half the frame, and the footer's modal-mode
//! hint shows `[C]opy`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::sync::Arc;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer as RtBuffer;

use rune_core::buffer::Buffer;
use rune_tui::app::{self, App};
use rune_tui::banner;
use rune_tui::clipboard::osc52_copy;
use rune_tui::footer;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::render;
use rune_tui::runtime::{Effects, Msg};
use rune_vfs::Mem;

const WIDTH: u16 = 80;
const HEIGHT: u16 = 24;

fn app_for(content: &str) -> App {
    let mut app = App::new(Buffer::new(content), None, Arc::new(Mem::new()), None);
    app.active_doc_mut().viewport.set_size(WIDTH, HEIGHT - 1);
    app.frame_height = HEIGHT;
    app.sync_view();
    app
}

fn draw(app: &App) -> RtBuffer {
    let backend = TestBackend::new(WIDTH, HEIGHT);
    let mut terminal = Terminal::new(backend).expect("terminal construction");
    terminal
        .draw(|frame| render::draw(app, frame))
        .expect("draw");
    terminal.backend().buffer().clone()
}

fn row_text(buf: &RtBuffer, y: u16, width: u16) -> String {
    let mut s = String::new();
    for x in 0..width {
        if let Some(cell) = buf.cell((x, y)) {
            s.push_str(cell.symbol());
        }
    }
    s
}

fn frame_text(buf: &RtBuffer) -> String {
    (0..HEIGHT).map(|y| row_text(buf, y, WIDTH)).collect()
}

fn key(code: KeyCode) -> Msg {
    Msg::Key(KeyInput {
        code,
        mods: Mods::NONE,
    })
}

/// `Msg::Error` opens the modal banner (plan WP3.S4) rather than writing
/// `status_message` — the whole routing chokepoint (`app::update_inner`'s
/// `Msg::Error` arm -> `banner::report_error`).
#[test]
fn msg_error_opens_the_modal_banner() {
    let mut app = app_for("hello");
    let mut effects = Effects::default();
    app::update(
        &mut app,
        Msg::Error("line1\n\nlong paragraph that wraps beyond one row".to_string()),
        &mut effects,
    );
    assert!(app.modal.is_some());
    assert!(
        app.status_message.is_none(),
        "Msg::Error must no longer write status_message directly"
    );
}

/// The banner renders somewhere above the footer row once a modal is up —
/// its headline text appears in the frame, and (since the frame is huge
/// relative to this short error) NOT on the footer's own row.
#[test]
fn banner_rows_render_above_the_footer() {
    let mut app = app_for("hello");
    let mut effects = Effects::default();
    app::update(
        &mut app,
        Msg::Error("boom\n\ndetail line".to_string()),
        &mut effects,
    );
    app.sync_view();

    let total_rows = app.modal.as_ref().expect("modal set").total_rows();
    assert!(total_rows > 0);

    let buf = draw(&app);
    let footer_row_text = row_text(&buf, HEIGHT - 1, WIDTH);
    assert!(
        !footer_row_text.contains("boom"),
        "the banner headline must not land on the footer's own row"
    );
    assert!(
        frame_text(&buf).contains("boom"),
        "expected the banner headline somewhere in the frame"
    );
}

/// A huge error's banner is capped at half the frame height (plan WP3.S3:
/// `height = min(total_rows, area.height / 2)`) — content past the cap is
/// simply not shown, rather than growing the banner (or shrinking the
/// editor) without bound.
#[test]
fn banner_height_caps_at_half_the_frame_for_a_huge_error() {
    let mut app = app_for("hello");
    let mut effects = Effects::default();
    let huge: String = (0..40)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    app::update(&mut app, Msg::Error(huge), &mut effects);
    app.sync_view();

    let total_rows = app.modal.as_ref().expect("modal set").total_rows();
    assert!(
        total_rows as u16 > HEIGHT / 2,
        "test setup: the error must produce more wrap rows than half the frame \
         (got {total_rows}, need > {})",
        HEIGHT / 2
    );

    let buf = draw(&app);
    let text = frame_text(&buf);
    assert!(
        text.contains("line 0"),
        "the start of the error must still be visible"
    );
    assert!(
        !text.contains("line 30"),
        "content past the half-frame cap must not render"
    );
}

/// PageDown's scroll delta must equal the ACTUALLY-rendered banner height —
/// review fix: previously `page_amount` read the modal document's own
/// `viewport.height`, which `sync_modal` never updated from the real
/// rendered height (`render::draw` computed its own `min(total_rows,
/// area.height / 2)` independently), so PageUp/PageDown paged by a stale
/// screenful that disagreed with what was actually on screen — shadow
/// state. `banner::banner_height` is now the one function both sides call.
#[test]
fn page_down_scrolls_by_the_actually_rendered_banner_height() {
    let mut app = app_for("hello");
    // Deliberately small frame: forces the half-frame cap well below the
    // error's own `total_rows`, so a stale (uncapped) `viewport.height`
    // would produce a visibly different scroll delta than the real one.
    app.frame_height = 10;
    let mut effects = Effects::default();
    let huge: String = (0..40)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    app::update(&mut app, Msg::Error(huge), &mut effects);
    app.sync_view();

    let expected = banner::banner_height(&app, app.frame_height);
    assert!(
        expected > 0,
        "test setup: the banner must actually be capped"
    );

    let mut effects2 = Effects::default();
    app::update(&mut app, key(KeyCode::PageDown), &mut effects2);

    let banner::Modal::Error(state) = app.modal.as_ref().expect("modal set") else {
        unreachable!("expected the Error modal");
    };
    assert_eq!(
        state.doc.viewport.scroll_row, expected as usize,
        "PageDown must scroll by exactly the rendered banner height"
    );
}

/// `c`/`C` (stage 1) copies the modal document's whole buffer via OSC 52
/// into `effects.raw` and clears the modal — plan WP3.S2/S5.
#[test]
fn pressing_c_copies_via_osc52_and_clears_the_modal() {
    let mut app = app_for("hello");
    let mut effects = Effects::default();
    app::update(&mut app, Msg::Error("boom".to_string()), &mut effects);
    assert!(app.modal.is_some());

    let mut effects2 = Effects::default();
    app::update(&mut app, key(KeyCode::Char('c')), &mut effects2);

    assert!(app.modal.is_none(), "'c' must clear the modal");
    assert_eq!(
        effects2.raw,
        vec![osc52_copy("\u{26A0} boom\n\n".as_bytes())],
        "'c' must emit exactly the modal buffer's bytes via OSC 52"
    );
}

/// `Esc` clears the modal without copying anything.
#[test]
fn escape_clears_the_modal_without_copying() {
    let mut app = app_for("hello");
    let mut effects = Effects::default();
    app::update(&mut app, Msg::Error("boom".to_string()), &mut effects);
    assert!(app.modal.is_some());

    let mut effects2 = Effects::default();
    app::update(&mut app, key(KeyCode::Escape), &mut effects2);

    assert!(app.modal.is_none());
    assert!(effects2.raw.is_empty(), "Esc must never emit OSC 52 bytes");
}

/// While a modal is up, a printable key is consumed at stage 1 — it never
/// reaches the editor's own buffer (plan WP3.S2/S5), and an unbound key (
/// from the modal's own perspective) leaves the modal untouched.
#[test]
fn a_printable_key_does_not_reach_the_editor_buffer_while_a_modal_is_up() {
    let mut app = app_for("hello");
    let id = app.active;
    let mut effects = Effects::default();
    app::update(&mut app, Msg::Error("boom".to_string()), &mut effects);
    assert!(app.modal.is_some());

    let before = app.doc(id).unwrap().buffer.content().to_string();
    let mut effects2 = Effects::default();
    app::update(&mut app, key(KeyCode::Char('x')), &mut effects2);

    assert_eq!(
        app.doc(id).unwrap().buffer.content(),
        before,
        "a key consumed by the modal must never reach commands::edit"
    );
    assert!(
        app.modal.is_some(),
        "an unbound key (not Esc/c/C) must leave the modal up"
    );
}

/// A quit chord pressed while a modal is up is consumed by stage 1, never
/// reaching stage 2's global `QuitChord` handling — plan WP3.S2: "quit
/// chords included; a guard/banner interposes on quit by design."
#[test]
fn quit_chord_is_consumed_by_the_modal_not_the_global_pipeline() {
    let mut app = app_for("hello");
    let mut effects = Effects::default();
    app::update(&mut app, Msg::Error("boom".to_string()), &mut effects);

    let mut effects2 = Effects::default();
    app::update(
        &mut app,
        Msg::Key(KeyInput {
            code: KeyCode::Char('d'),
            mods: Mods {
                ctrl: true,
                alt: true,
                ..Mods::NONE
            },
        }),
        &mut effects2,
    );

    assert!(
        app.pending_quit.is_none(),
        "modal capture must consume the quit chord before stage 2 ever arms pending_quit"
    );
    assert!(app.modal.is_some());
}

/// The footer's modal-mode hint shows `[C]opy` (plan WP3.S3), outranking
/// every other footer mode.
#[test]
fn footer_shows_copy_hint_in_modal_mode() {
    let mut app = app_for("hello");
    let mut effects = Effects::default();
    app::update(&mut app, Msg::Error("boom".to_string()), &mut effects);

    let text = footer::footer_text(&app);
    assert!(
        text.contains("[C]opy"),
        "expected '[C]opy' in the modal-mode footer text, got {text:?}"
    );
}
