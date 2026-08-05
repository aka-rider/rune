//! WP2: regression coverage for link/embed resolution's "one mechanism"
//! requirement — a followable link and an image
//! embed sharing the same raw target text must resolve through identical
//! policy, and `sync_embeds`'s dedupe of same-target duplicates must be
//! deterministic rather than keyed off arbitrary `HashMap` iteration order.
//! Kept separate from `inline_embed.rs` (WP9's own file, already near the
//! §1.6 budget) rather than grown into it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::Path;
use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_core::cursor::CursorSet;
use rune_nav::{Destination, RefKind, UseRole};
use rune_tui::app::{App, update};
use rune_tui::document::DocumentId;
use rune_tui::runtime::{CmdKind, Effects, Msg};
use rune_vfs::{Mem, Vfs};

const X_PNG: &[u8] = include_bytes!("../../../testdata/assets/x.png");

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
/// resize) and decodes whatever it spawned.
fn discover_and_decode(app: &mut App) {
    let mut effects = Effects::default();
    update(app, Msg::Resize(60, 20), &mut effects);
    run_decodes(app, effects);
    app.sync_view();
}

/// Builds an `App` bound to `/vault/doc.md` over a fresh `Mem` vfs seeded
/// with `x.png` at every path in `seed_paths`, with the caret parked past
/// `content`'s end so the very first `sync_view` sees a Rendered (not
/// revealed) embed line.
fn app_with_content(content: &str, seed_paths: &[&str]) -> (App, DocumentId) {
    let mem = Arc::new(Mem::new());
    for p in seed_paths {
        mem.save_atomic(Path::new(p), X_PNG).expect("seed image");
    }
    let vfs: Arc<dyn Vfs + Send + Sync> = mem;
    let mut app = App::new(
        Buffer::new(content),
        Some(Path::new("/vault/doc.md").to_path_buf()),
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

/// WP2.S1/S3 regression guard: a followable link and an image embed
/// pointing at the same relative target must resolve to the same absolute
/// path — the "one mechanism" requirement, so `navigate::follow`'s resolver
/// and `sync_embeds`'s can never drift onto different policy again. Uses a
/// subdirectory target (`assets/pic.png`), unlike every fixture in
/// `inline_embed.rs`, which places the document and its image side by side.
#[test]
fn a_link_and_an_embed_with_the_same_relative_target_resolve_to_the_same_path() {
    let content = "[see picture](assets/pic.png)\n\n![alt](assets/pic.png)\n";
    let (mut app, id) = app_with_content(content, &["/vault/assets/pic.png"]);

    discover_and_decode(&mut app);

    let doc = app.doc(id).expect("doc");
    let embed = doc
        .embeds
        .images
        .get("assets/pic.png")
        .expect("embed tracked");
    let embed_path = embed.abs_path.clone();

    let link_target = doc
        .catalogue
        .iter()
        .find_map(|r| match &r.kind {
            RefKind::Use {
                role: UseRole::Link,
                target,
            } => Some(target.clone()),
            _ => None,
        })
        .expect("a link Ref must be in the catalogue");

    let doc_dir = doc
        .file_path
        .as_deref()
        .and_then(Path::parent)
        .map(Path::to_path_buf);
    let dest = rune_nav::resolve(
        app.vfs.as_ref(),
        &link_target,
        doc_dir.as_deref(),
        &app.root,
        rune_md::catalogue::NAME_RESOLUTION_EXTENSION,
    );
    match dest {
        Destination::Location { path, .. } => {
            assert_eq!(
                path, embed_path,
                "link following and the embed reconciler resolved the same \
                 target to different paths"
            );
        }
        other => panic!("link target did not resolve: {other:?}"),
    }
}

/// WP2.S2 regression guard: `sync_embeds`'s dedupe used to key off an
/// arbitrary `HashMap` iteration order, so which of two same-target embeds
/// survived (a markdown `![alt](x.png)` and a wikilink `![[x.png]]`) was
/// random per run — sorting by line before dedupe fixes it. Looped 25 times
/// (fresh `App`, fresh `Mem` each time) so a reintroduced hash-order flake
/// would show up as a mismatch against the first iteration's result.
#[test]
fn the_same_target_in_two_forms_produces_the_same_tracked_embed_across_repeated_runs() {
    let content = "![alt](x.png)\n\n![[x.png]]\n";
    let mut first: Option<std::path::PathBuf> = None;
    for _ in 0..25 {
        let (mut app, id) = app_with_content(content, &["/vault/x.png"]);
        discover_and_decode(&mut app);

        let doc = app.doc(id).expect("doc");
        assert_eq!(
            doc.embeds.images.len(),
            1,
            "both forms name the same target and must collapse to one tracked embed"
        );
        let embed = doc.embeds.images.get("x.png").expect("embed tracked");
        match &first {
            None => first = Some(embed.abs_path.clone()),
            Some(expected) => assert_eq!(
                &embed.abs_path, expected,
                "the surviving embed's resolved path must be the same every run"
            ),
        }
    }
}
