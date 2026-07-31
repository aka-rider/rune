//! Copy, cut, and paste in the title — split out of `rename_bind.rs`
//! (plan WP5, §1.6). Added by the clipboard package that grew that file
//! past the ceiling.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod rename_common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rune_tui::app::App;
use rune_tui::clipboard::osc52_copy;
use rune_tui::keymap::KeyCode;
use rune_tui::runtime::{Msg, PasteTarget};

use rune_core::buffer::Buffer;
use rune_vfs::{Mem, Vfs};

use rename_common::{app_with, ctrl, plain, seeded_vfs, send, sup};

/// Assumption A2: with the gate locked, ⌘C copies the WINDOW (the stem
/// alone), never the whole name — and never mutates the document buffer,
/// which is the `PANE-NO-BLEED` property applied to the title's own
/// clipboard commands.
#[test]
fn cmd_c_in_the_title_copies_the_window_not_the_whole_name() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    let before = app.active_doc().buffer.content().to_string();

    send(&mut app, ctrl('r'));
    assert_eq!(app.title.text(), "a.md");
    assert!(
        !app.title.ext_unlocked(),
        "seeded with a stem: starts locked"
    );

    let effects = send(&mut app, sup('c'));

    assert_eq!(
        effects.raw,
        vec![osc52_copy(b"a")],
        "locked, ⌘C must copy the stem alone, never 'a.md'"
    );
    assert_eq!(
        app.active_doc().buffer.content(),
        before,
        "copying the title must never touch the document buffer"
    );
}

/// Assumption A2's own regression: ⌘C then ⌘X then ⌘V must round-trip the
/// name unchanged, which is only possible when copy and cut act on the
/// IDENTICAL range. If copy took the whole name while cut could only
/// delete the window, ⌘X would leave the extension behind and pasting the
/// (whole-name) copy back would double it (`lessrc.md.md`).
#[test]
fn cmd_c_then_cmd_x_then_cmd_v_round_trips_the_name_unchanged() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(Path::new("/root/lessrc.md"), b"body")
        .expect("seed lessrc.md");
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(&mem) as Arc<dyn Vfs + Send + Sync>;
    let mut app = App::new(
        Buffer::new("body"),
        Some(PathBuf::from("/root/lessrc.md")),
        vfs,
        None,
    );

    send(&mut app, ctrl('r'));
    assert_eq!(app.title.text(), "lessrc.md");
    assert!(!app.title.ext_unlocked());
    // The oracle: what the window covers right now, independent of
    // whatever `title::keys` internally does with it.
    let expected = app
        .title
        .text()
        .get(app.title.window())
        .expect("a valid window")
        .to_string();
    assert_eq!(expected, "lessrc");

    let copy_effects = send(&mut app, sup('c'));
    assert_eq!(
        copy_effects.raw,
        vec![osc52_copy(expected.as_bytes())],
        "⌘C must copy exactly the window"
    );

    send(&mut app, sup('x'));
    assert_eq!(
        app.title.text(),
        ".md",
        "⌘X must delete exactly the range ⌘C just copied"
    );

    // Simulate the pbpaste reply carrying exactly what was copied (never
    // actually shelling out to a real pbpaste in a test — the routing this
    // proves is `PasteTarget::Title`, not the subprocess itself).
    send(
        &mut app,
        Msg::ClipboardRead {
            text: expected,
            target: PasteTarget::Title,
        },
    );

    assert_eq!(
        app.title.text(),
        "lessrc.md",
        "⌘V must restore exactly what ⌘C/⌘X took, proving A2's ranges agree"
    );
}

/// A `ClipboardRead` targeted at the title inserts SANITIZED text into the
/// field: only the first line survives, and control characters/`/` (an
/// `INVALID_NAME_CHARS` entry) are dropped — the same restrictions ordinary
/// typing enforces one `char` at a time, applied at once to a paste.
#[test]
fn a_clipboard_read_targeted_at_the_title_inserts_filtered_text_into_the_field() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);

    send(&mut app, ctrl('r'));
    send(&mut app, ctrl('a'));
    send(&mut app, plain(KeyCode::Backspace));
    assert_eq!(
        app.title.text(),
        ".md",
        "the stem is cleared, gate still locked"
    );

    send(
        &mut app,
        Msg::ClipboardRead {
            text: "evil/name\nsecond line".to_string(),
            target: PasteTarget::Title,
        },
    );

    assert_eq!(
        app.title.text(),
        "evilname.md",
        "only the first line survives, and '/' is dropped"
    );
}
