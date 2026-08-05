//! WP3: regression coverage for the exact reported bug — a document whose
//! own directory lies OUTSIDE `app.root`. Every fixture in
//! `inline_embed.rs` and `embed_link_resolution.rs` places the document
//! inside the root, which is
//! exactly why the vault-containment check WP1 removed could break every
//! relative reference in an out-of-root document without any existing test
//! catching it. Kept as its own file rather than grown into either of
//! those, both already near the §1.6 budget.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::Path;
use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_core::cursor::CursorSet;
use rune_tui::app::{App, update};
use rune_tui::document::DocumentId;
use rune_tui::graphics::ImageStatus;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::runtime::{CmdKind, Effects, Msg};
use rune_vfs::{Mem, Vfs};

const X_PNG: &[u8] = include_bytes!("../../../testdata/assets/x.png");

fn sup_enter() -> KeyInput {
    KeyInput {
        code: KeyCode::Enter,
        mods: Mods {
            sup: true,
            ..Mods::NONE
        },
    }
}

/// Builds an `App` bound to `/elsewhere/doc.md` — a directory that is NOT
/// under `app.root` (`/vault`), the exact shape the reported bug needed:
/// `resolve_candidate`'s old containment check refused `doc_dir` itself
/// whenever it sat outside `root`, so every relative reference in a
/// document like this one failed to resolve regardless of what the
/// reference actually named. `mem` is the caller's own `Mem` vfs, already
/// seeded with whatever sibling files the fixture needs — the caret is
/// parked past `content`'s end for the same reason `inline_embed.rs`'s own
/// `app_with_embed` does: a cursor inside a standalone image's own byte
/// range would reveal it before the first reconcile pass ever runs.
fn app_with_content(mem: Arc<Mem>, content: &str) -> (App, DocumentId) {
    let vfs: Arc<dyn Vfs + Send + Sync> = mem;
    let mut app = App::new(
        Buffer::new(content),
        Some(Path::new("/elsewhere/doc.md").to_path_buf()),
        vfs,
        None,
    );
    let id = app.active;
    app.graphics.kitty = true;
    app.root = Path::new("/vault").to_path_buf();
    let end = content.len();
    app.doc_mut(id).expect("doc").cursors = CursorSet::new(end);
    app.sync_view();
    (app, id)
}

/// Runs every spawned `CmdKind::ImageDecode` synchronously and feeds each
/// reply back through `update`, the same shape `inline_embed.rs`'s own
/// `run_decodes` uses.
fn run_decodes(app: &mut App, effects: Effects) {
    for cmd in effects.cmds {
        if cmd.kind() != CmdKind::ImageDecode {
            continue;
        }
        if let Some(msg) = cmd.run() {
            let mut reply_effects = Effects::default();
            update(app, msg, &mut reply_effects);
        }
    }
}

/// Drives the post-dispatch `sync_embeds` chokepoint once (via a harmless
/// resize) and decodes whatever it spawned — the same shape both sibling
/// test files use.
fn discover_and_decode(app: &mut App) {
    let mut effects = Effects::default();
    update(app, Msg::Resize(60, 20), &mut effects);
    run_decodes(app, effects);
    app.sync_view();
}

/// WP3.S1: the exact reported scenario — `![[Do not try to DRY.png]]`
/// stripped to its essential shape (a bare-basename wiki embed, no
/// directory component) in a document whose directory is outside
/// `app.root`. Before WP1, `resolve_candidate` refused `doc_dir` itself
/// because `doc_dir` (`/elsewhere`) does not start with `root` (`/vault`),
/// so `root` was the only base tried and the bare basename never resolved
/// there either — the embed stayed `Failed`, never `Live`.
#[test]
fn a_bare_basename_wiki_embed_in_an_out_of_root_document_reaches_live() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(Path::new("/elsewhere/image.png"), X_PNG)
        .expect("seed image.png");

    let content = "![[image.png]]\n";
    let (mut app, id) = app_with_content(mem, content);

    discover_and_decode(&mut app);

    let doc = app.doc(id).expect("doc");
    let embed = doc.embeds.images.get("image.png").expect("embed tracked");
    assert_eq!(
        embed.status,
        ImageStatus::Live,
        "a bare-basename wiki embed in a document outside app.root must \
         still resolve against the document's own directory"
    );
    assert_eq!(
        embed.abs_path,
        Path::new("/elsewhere/image.png"),
        "must resolve against doc_dir, not root"
    );
}

/// WP3.S2: the sibling case for link following — a relative link in the
/// same out-of-root document must resolve to `Destination::Location`
/// rather than `Destination::Unresolved`. Driven through the real
/// ⌘Enter-follow path (`navigate.rs`'s own style) rather than calling
/// `rune_nav::resolve` directly, so the assertion covers the whole
/// follow-a-link chokepoint, not just the resolver in isolation: opening a
/// new tab bound to the target's absolute path is only possible if
/// `navigate::follow` received `Destination::Location` back.
#[test]
fn a_relative_link_in_an_out_of_root_document_opens_its_target() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(Path::new("/elsewhere/note.md"), b"note body\n")
        .expect("seed note.md");

    let content = "[see note](note.md)\n";
    let (mut app, id) = app_with_content(mem, content);
    let before = app.documents.len();

    let offset = content.find("note.md").expect("fixture has link");
    app.doc_mut(id).expect("doc").cursors = CursorSet::new(offset);

    let mut effects = Effects::default();
    update(&mut app, Msg::Key(sup_enter()), &mut effects);
    for cmd in effects.cmds.drain(..) {
        assert_eq!(
            cmd.kind(),
            CmdKind::ReadFile,
            "expected only a ReadFile Cmd"
        );
        if let Some(msg) = cmd.run() {
            let mut inner = Effects::default();
            update(&mut app, msg, &mut inner);
        }
    }
    app.sync_view();

    assert_eq!(
        app.documents.len(),
        before + 1,
        "a relative link in an out-of-root document must resolve and open"
    );
    assert_eq!(
        app.active_doc().file_path.as_deref(),
        Some(Path::new("/elsewhere/note.md")),
        "must resolve against doc_dir, not root"
    );
}
