//! The `Keyboard::QueryFlags` reply's consumption seam: `Msg::
//! KeyboardFlagsReport` settles `App::keyboard_flags` from `None` to a real
//! answer, and posts a one-time note only when that answer confirms the
//! terminal dropped a bit this app asked for.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod tui_edit_common;

use termina::escape::csi::KittyKeyboardFlags;

use rune_tui::runtime::{Effects, Msg};
use tui_edit_common::app_for;

fn report(app: &mut rune_tui::app::App, flags: KittyKeyboardFlags) {
    let mut effects = Effects::default();
    rune_tui::app::update(app, Msg::KeyboardFlagsReport(flags), &mut effects);
}

#[test]
fn a_reply_confirming_both_requested_bits_lands_silently() {
    let mut app = app_for("", 0);
    let posts_before = rune_tui::messages::posts(&app);

    report(
        &mut app,
        KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES | KittyKeyboardFlags::REPORT_ALTERNATE_KEYS,
    );

    assert_eq!(
        app.keyboard_flags,
        Some(
            KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES
                | KittyKeyboardFlags::REPORT_ALTERNATE_KEYS
        )
    );
    assert_eq!(rune_tui::messages::posts(&app), posts_before);
}

#[test]
fn a_reply_missing_a_requested_bit_posts_one_warning() {
    let mut app = app_for("", 0);

    report(&mut app, KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES);
    assert_eq!(
        app.keyboard_flags,
        Some(KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES)
    );
    let posts_after_first = rune_tui::messages::posts(&app);
    assert!(
        rune_tui::messages::newest_text(&app).is_some_and(|m| m.contains("disambiguation")),
        "expected a disambiguation warning, got {:?}",
        rune_tui::messages::newest_text(&app)
    );

    report(&mut app, KittyKeyboardFlags::DISAMBIGUATE_ESCAPE_CODES);
    assert_eq!(
        rune_tui::messages::posts(&app),
        posts_after_first,
        "the same warning must not repost on a duplicate reply"
    );
}
