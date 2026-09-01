//! Tests for following the link under the cursor via
//! ⌘Enter/^Enter and via a ctrl-click, same-document heading jumps,
//! cross-document anchors, external URLs, and the broken-link/embed
//! no-ops. Headless (`Mem` vfs, no real terminal), mirroring
//! `tests/explorer.rs`'s own seeding style.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_core::coords::BufferOffset;
use rune_core::cursor::CursorSet;
use rune_nav::RefKind;
use rune_tui::app::{self, App};
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::pointer::{MouseButton, MouseInput, MouseKind};
use rune_tui::runtime::{CmdKind, Effects, Msg};
use rune_vfs::{Mem, Vfs, VfsTestExt};

/// Following a link can post a message, and an open message pane arms its own
/// auto-collapse timer — so "nothing was opened" is a claim about the external
/// handler specifically, never about the effect list being empty. Keeping it
/// scoped to `OpenExternal` is what makes the `javascript:`/`file://` cases
/// still assert the thing that matters: that neither ever reaches the OS
/// opener.
fn opens_externally(effects: &Effects) -> bool {
    effects
        .cmds
        .iter()
        .any(|cmd| cmd.kind() == CmdKind::OpenExternal)
}

const WIDTH: u16 = 80;
const HEIGHT: u16 = 24;

fn app_with(mem: &Arc<Mem>, path: &str, content: &str) -> App {
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(mem) as Arc<dyn Vfs + Send + Sync>;
    let mut app = App::new(
        Buffer::new(content),
        Some(
            rune_tui::resolved::ResolvedPath::resolve(
                vfs.as_ref(),
                std::path::Path::new(&PathBuf::from(path)),
            )
            .expect("the launch path resolves"),
        ),
        vfs,
        None,
    );
    app.set_root(PathBuf::from("/root"));
    // `App::frame` (not a direct `viewport.set_size`) is what
    // `layout::geometry` — and so the mouse click tests' own
    // `editor_origin` — actually reads; `sync_view`'s `relayout` derives
    // the viewport size from it the same way the real runtime does.
    app.frame = Some(rune_tui::app::FrameSize::new(WIDTH, HEIGHT));
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
    let area = app.frame_area();
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

/// Runs every `ReadFile` `Cmd` `effects` carries inline and feeds its
/// `Msg::FileOpened` reply straight back through `update` — models one
/// whole runtime cycle for `workspace::open_path_async`
/// without spawning a real thread. Panics on any OTHER `Cmd` kind: a test
/// using this helper is asserting "this key opens a file", and a surprise
/// second kind of `Cmd` would mean the assertion no longer describes what
/// actually happened.
fn settle_file_opens(app: &mut App, mut effects: Effects) {
    for cmd in effects.cmds.drain(..) {
        assert_eq!(
            cmd.kind(),
            CmdKind::ReadFile,
            "expected only a ReadFile Cmd"
        );
        if let Some(msg) = cmd.run() {
            let mut inner = Effects::default();
            app::update(app, msg, &mut inner);
            assert!(
                inner.cmds.is_empty(),
                "Msg::FileOpened must not itself spawn a Cmd"
            );
        }
    }
    app.sync_view();
}

/// `press` + [`settle_file_opens`] — the async counterpart of a plain
/// `press` for keys expected to open a file off-thread.
fn press_and_open(app: &mut App, key: KeyInput) {
    let effects = press(app, key);
    settle_file_opens(app, effects);
}

/// `click` + [`settle_file_opens`] — the async counterpart of a plain
/// `click` for gestures expected to open a file off-thread.
fn click_and_open(app: &mut App, col: u16, row: u16, ctrl: bool) {
    let effects = click(app, col, row, ctrl);
    settle_file_opens(app, effects);
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

    press_and_open(&mut app, sup_enter());

    assert_eq!(app.documents.len(), before + 1);
    assert_eq!(app.active_doc().path(), Some(Path::new("/root/note.md")));
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

    press_and_open(&mut app, ctrl_enter());

    assert_eq!(app.documents.len(), before + 1);
    assert_eq!(app.active_doc().path(), Some(Path::new("/root/note.md")));
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

    click_and_open(&mut app, note_col, 0, true);
    assert_eq!(app.documents.len(), 2, "ctrl-click must open the target");
    assert_eq!(app.active_doc().path(), Some(Path::new("/root/note.md")));

    // Switch back and confirm a PLAIN double-click still selects a word —
    // the ctrl-click gesture above must never displace the ordinary
    // multi-click run.
    app.active = original;
    click(&mut app, 1, 0, false);
    click(&mut app, 1, 0, false);
    let c = app.active_doc().cursors.primary();
    assert_eq!(
        c.selection_range(),
        (BufferOffset(0), BufferOffset(5)),
        "expected \"hello\" selected"
    );
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

    press_and_open(&mut app, sup_enter());

    assert_eq!(app.active_doc().path(), Some(Path::new("/root/note.md")));
    assert_eq!(app.active_doc().cursors.primary().position.get(), expected);
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
    assert_eq!(app.active_doc().cursors.primary().position.get(), expected);
}

#[test]
fn a_broken_link_posts_a_message_and_opens_nothing() {
    let mem = Arc::new(Mem::new());
    let content = "[x](./missing.md)\n";
    let mut app = app_with(&mem, "/root/a.md", content);
    let before = app.documents.len();
    place_cursor(&mut app, content.find("missing").expect("fixture has link"));

    let effects = press(&mut app, sup_enter());

    assert_eq!(app.documents.len(), before, "a broken link opens nothing");
    assert!(!opens_externally(&effects), "a broken link opens nothing");
    assert!(
        rune_tui::messages::newest_text(&app).is_some(),
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
        !opens_externally(&effects),
        "javascript: must never reach the OS opener"
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

    assert!(
        !opens_externally(&effects),
        "file:// must never reach the OS opener"
    );
    assert_eq!(app.documents.len(), before);
}

#[test]
fn an_embed_ref_is_never_followed() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(Path::new("/root/note.md"), b"note body\n")
        .expect("seed note.md");
    // Real markdown image syntax — `rune-md::catalogue`'s `Inline::Image`
    // arm emits a genuine `UseRole::Embed` `Ref` for it (pinned by its own
    // `markdown_image_becomes_an_embed_use` test), so this reaches
    // `navigate::follow`'s embed skip through the real parser pipeline
    // rather than a hand-built catalogue entry.
    let content = "![alt](note.md)\n";
    let mut app = app_with(&mem, "/root/a.md", content);
    let before = app.documents.len();
    place_cursor(
        &mut app,
        content.find("note.md").expect("fixture has target"),
    );

    let effects = press(&mut app, sup_enter());

    assert_eq!(
        app.documents.len(),
        before,
        "an embed must never be followed"
    );
    assert!(effects.cmds.is_empty());
}

#[test]
fn a_caret_at_the_links_own_range_end_still_follows_it() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(Path::new("/root/url.md"), b"target body\n")
        .expect("seed url.md");
    let content = "[text](url)\n";
    let mut app = app_with(&mem, "/root/a.md", content);
    place_cursor(&mut app, "[text](url)".len());

    press_and_open(&mut app, ctrl_enter());

    assert_eq!(
        app.active_doc().path(),
        Some(Path::new("/root/url.md")),
        "a caret at the link's own range end must still resolve a Destination"
    );
}
