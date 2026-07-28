//! WP5 "Done when" tests: following the link under the cursor via
//! ⌘Enter/^Enter and via a ctrl-click, same-document heading jumps,
//! cross-document anchors, external URLs, and the broken-link/embed
//! no-ops. Headless (`Mem` vfs, no real terminal), mirroring
//! `tests/explorer.rs`'s own seeding style.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_core::cursor::CursorSet;
use rune_nav::{Ref, RefKind, Target, UseRole};
use rune_syntax::element::ByteRange;
use rune_tui::app::{self, App};
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::pointer::{MouseButton, MouseInput, MouseKind};
use rune_tui::runtime::{CmdKind, Effects, Msg};
use rune_vfs::{Mem, Vfs};

const WIDTH: u16 = 80;
const HEIGHT: u16 = 24;

fn app_with(mem: &Arc<Mem>, path: &str, content: &str) -> App {
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(mem) as Arc<dyn Vfs + Send + Sync>;
    let mut app = App::new(Buffer::new(content), Some(PathBuf::from(path)), vfs, None);
    app.set_root(PathBuf::from("/root"));
    // `frame_width`/`frame_height` (not a direct `viewport.set_size`) are
    // what `layout::geometry` — and so the mouse click tests' own
    // `editor_origin` — actually reads; `sync_view`'s `relayout` derives
    // the viewport size from them the same way the real runtime does.
    app.frame_width = WIDTH;
    app.frame_height = HEIGHT;
    app.sync_view();
    app
}

fn place_cursor(app: &mut App, offset: usize) {
    app.active_doc_mut().cursors = CursorSet::new(offset);
}

fn sup_enter() -> KeyInput {
    KeyInput {
        code: KeyCode::Enter,
        mods: Mods {
            sup: true,
            ..Mods::NONE
        },
    }
}

fn ctrl_enter() -> KeyInput {
    KeyInput {
        code: KeyCode::Enter,
        mods: Mods {
            ctrl: true,
            ..Mods::NONE
        },
    }
}

/// Sends one `Msg::Key` through the real `update`, resyncing afterward
/// (what the runtime does once per whole message batch) so a later
/// assertion sees the settled state. Returns the `Effects` the key
/// produced, before that resync ran.
fn press(app: &mut App, key: KeyInput) -> Effects {
    let mut effects = Effects::default();
    app::update(app, Msg::Key(key), &mut effects);
    app.sync_view();
    effects
}

/// `(col, row)` relative to the editor rect, translated to absolute frame
/// coordinates — mirrors `commands::mouse`'s own test helper.
fn editor_origin(app: &App) -> (u16, u16) {
    let area = ratatui::layout::Rect::new(0, 0, app.frame_width, app.frame_height);
    let editor = rune_tui::layout::geometry(area, app).editor;
    (editor.x, editor.y)
}

fn click(app: &mut App, col: u16, row: u16, ctrl: bool) -> Effects {
    let (ox, oy) = editor_origin(app);
    let mut effects = Effects::default();
    app::update(
        app,
        Msg::Mouse(MouseInput {
            kind: MouseKind::Down(MouseButton::Left),
            column: ox + col,
            row: oy + row,
            shift: false,
            alt: false,
            ctrl,
        }),
        &mut effects,
    );
    app.sync_view();
    effects
}

/// Ground truth for a heading's own byte offset: parses `content` through
/// the real production pipeline (`rune_md::parse`/`catalogue`) rather than
/// hardcoding a byte count that would silently rot if the fixture changed.
fn heading_offset(content: &str, heading: &str) -> usize {
    let blocks = rune_md::parse::parse(content);
    let catalogue = rune_md::catalogue::catalogue(content, &blocks);
    catalogue
        .iter()
        .find_map(|r| match &r.kind {
            RefKind::Def { name, .. } if name == heading => Some(r.site.start),
            _ => None,
        })
        .expect("fixture has the expected heading")
}

#[test]
fn super_enter_follows_a_wikilink_into_a_new_tab() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(Path::new("/root/note.md"), b"note body\n")
        .expect("seed note.md");
    let content = "[[note]]\n";
    let mut app = app_with(&mem, "/root/a.md", content);
    let before = app.documents.len();
    place_cursor(&mut app, content.find("note").expect("fixture has note"));

    press(&mut app, sup_enter());

    assert_eq!(app.documents.len(), before + 1);
    assert_eq!(
        app.active_doc().file_path.as_deref(),
        Some(Path::new("/root/note.md"))
    );
}

#[test]
fn ctrl_enter_follows_a_wikilink_into_a_new_tab() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(Path::new("/root/note.md"), b"note body\n")
        .expect("seed note.md");
    let content = "[[note]]\n";
    let mut app = app_with(&mem, "/root/a.md", content);
    let before = app.documents.len();
    place_cursor(&mut app, content.find("note").expect("fixture has note"));

    press(&mut app, ctrl_enter());

    assert_eq!(app.documents.len(), before + 1);
    assert_eq!(
        app.active_doc().file_path.as_deref(),
        Some(Path::new("/root/note.md"))
    );
}

#[test]
fn ctrl_click_follows_a_link_while_a_plain_double_click_still_selects_a_word() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(Path::new("/root/note.md"), b"note body\n")
        .expect("seed note.md");
    let content = "hello [[note]] world\n";
    let mut app = app_with(&mem, "/root/a.md", content);
    let original = app.active;
    let note_col = content.find("note").expect("fixture has note") as u16;

    click(&mut app, note_col, 0, true);
    assert_eq!(app.documents.len(), 2, "ctrl-click must open the target");
    assert_eq!(
        app.active_doc().file_path.as_deref(),
        Some(Path::new("/root/note.md"))
    );

    // Switch back and confirm a PLAIN double-click still selects a word —
    // the ctrl-click gesture above must never displace the ordinary
    // multi-click run.
    app.active = original;
    click(&mut app, 1, 0, false);
    click(&mut app, 1, 0, false);
    let c = app.active_doc().cursors.primary();
    assert_eq!(c.selection_range(), (0, 5), "expected \"hello\" selected");
}

#[test]
fn wikilink_with_anchor_lands_the_caret_on_the_headings_byte_offset() {
    let mem = Arc::new(Mem::new());
    let note_content = "intro\n\n# Setup\nbody\n";
    mem.save_atomic(Path::new("/root/note.md"), note_content.as_bytes())
        .expect("seed note.md");
    let expected = heading_offset(note_content, "Setup");

    let content = "[[note#Setup]]\n";
    let mut app = app_with(&mem, "/root/a.md", content);
    place_cursor(&mut app, content.find("note").expect("fixture has note"));

    press(&mut app, sup_enter());

    assert_eq!(
        app.active_doc().file_path.as_deref(),
        Some(Path::new("/root/note.md"))
    );
    assert_eq!(app.active_doc().cursors.primary().position, expected);
}

#[test]
fn hash_link_jumps_within_the_same_document() {
    let content = "[x](#Setup)\n\nintro\n\n# Setup\nbody\n";
    let expected = heading_offset(content, "Setup");
    let mem = Arc::new(Mem::new());
    let mut app = app_with(&mem, "/root/a.md", content);
    let before = app.documents.len();
    place_cursor(&mut app, content.find("Setup").expect("fixture has link"));

    press(&mut app, sup_enter());

    assert_eq!(
        app.documents.len(),
        before,
        "must stay in the same document"
    );
    assert_eq!(app.active_doc().cursors.primary().position, expected);
}

#[test]
fn a_broken_link_sets_a_status_message_and_opens_nothing() {
    let mem = Arc::new(Mem::new());
    let content = "[x](./missing.md)\n";
    let mut app = app_with(&mem, "/root/a.md", content);
    let before = app.documents.len();
    place_cursor(&mut app, content.find("missing").expect("fixture has link"));

    let effects = press(&mut app, sup_enter());

    assert_eq!(app.documents.len(), before, "a broken link opens nothing");
    assert!(effects.cmds.is_empty());
    assert!(
        app.status_message.is_some(),
        "a broken link must set a status message"
    );
}

#[test]
fn an_external_link_produces_exactly_one_open_external_cmd() {
    let mem = Arc::new(Mem::new());
    let content = "[x](https://example.com)\n";
    let mut app = app_with(&mem, "/root/a.md", content);
    place_cursor(&mut app, content.find("https").expect("fixture has link"));

    let effects = press(&mut app, sup_enter());

    assert_eq!(effects.cmds.len(), 1);
    assert_eq!(effects.cmds[0].kind(), CmdKind::OpenExternal);
}

#[test]
fn a_javascript_link_produces_no_cmd_and_resolves_to_unresolved() {
    let mem = Arc::new(Mem::new());
    let content = "[x](javascript:alert(1))\n";
    let mut app = app_with(&mem, "/root/a.md", content);
    let before = app.documents.len();
    place_cursor(
        &mut app,
        content.find("javascript").expect("fixture has link"),
    );

    let effects = press(&mut app, sup_enter());

    assert!(
        effects.cmds.is_empty(),
        "javascript: must never spawn a Cmd"
    );
    assert_eq!(app.documents.len(), before);
}

#[test]
fn a_file_scheme_link_produces_no_cmd_and_resolves_to_unresolved() {
    let mem = Arc::new(Mem::new());
    let content = "[x](file:///etc/passwd)\n";
    let mut app = app_with(&mem, "/root/a.md", content);
    let before = app.documents.len();
    place_cursor(&mut app, content.find("file").expect("fixture has link"));

    let effects = press(&mut app, sup_enter());

    assert!(effects.cmds.is_empty(), "file:// must never spawn a Cmd");
    assert_eq!(app.documents.len(), before);
}

#[test]
fn an_embed_ref_is_never_followed() {
    let mem = Arc::new(Mem::new());
    // Injects a synthetic `Embed` `Ref` directly onto the catalogue
    // (rather than relying on markdown image syntax, which this parser
    // does not model as a navigable `Ref` at all — see
    // `rune-md::catalogue`'s own pinned `embed_prefixed_wikilink_comrak_
    // behaviour_is_pinned` test): this is the one thing under test here,
    // `navigate::follow` must skip a `UseRole::Embed` hit under the cursor.
    let mut app = app_with(&mem, "/root/a.md", "placeholder\n");
    let before = app.documents.len();
    app.active_doc_mut().catalogue = vec![Ref {
        site: ByteRange::new(0, 11),
        kind: RefKind::Use {
            role: UseRole::Embed,
            target: Target::Path {
                path: "note.md".to_string(),
                anchor: None,
            },
        },
    }];
    place_cursor(&mut app, 2);

    let effects = press(&mut app, sup_enter());

    assert_eq!(
        app.documents.len(),
        before,
        "an embed must never be followed"
    );
    assert!(effects.cmds.is_empty());
}
