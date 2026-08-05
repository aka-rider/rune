//! An embed on its own LINE inside a larger paragraph — prose directly above
//! and below with no blank line separating them — must still render as an
//! image. Qualification is per line, not per paragraph, matching the Go
//! reference's own `isStandaloneImageLine`.
//!
//! Every other embed fixture in this crate puts a blank line either side, so
//! the paragraph-scoped predicate that used to reject this layout went
//! unnoticed: the embed silently rendered as its own target text and was never
//! decoded at all.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::Path;
use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_tui::app::{App, update};
use rune_tui::graphics::ImageStatus;
use rune_tui::runtime::{CmdKind, Effects, Msg};
use rune_vfs::{Mem, Vfs};

const X_PNG: &[u8] = include_bytes!("../../../testdata/assets/x.png");

/// A document at `/notes/t.md` whose workspace root is `/vault` — i.e. the
/// document lives OUTSIDE the root, the shape that previously broke relative
/// resolution — holding `content`, with `image.png` sitting beside it.
fn app_with(content: &str) -> (App, rune_tui::document::DocumentId) {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(Path::new("/notes/image.png"), X_PNG)
        .expect("seed image.png");
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(&mem) as Arc<dyn Vfs + Send + Sync>;
    let mut app = App::new(
        Buffer::new(content),
        Some(Path::new("/notes/t.md").to_path_buf()),
        vfs,
        None,
    );
    app.root = Path::new("/vault").to_path_buf();
    app.graphics.kitty = true;
    let id = app.active;
    app.active_doc_mut().viewport.set_size(80, 24);
    app.sync_view();
    (app, id)
}

/// Sends a harmless message so the post-dispatch chokepoint reconciles the
/// embed set, then runs whatever decodes it spawned and feeds each reply back
/// through the real loop — the same shape the other embed suites use.
fn discover_and_decode(app: &mut App) {
    let mut effects = Effects::default();
    update(app, Msg::Resize(80, 24), &mut effects);
    for cmd in effects.cmds {
        if cmd.kind() != CmdKind::ImageDecode {
            continue;
        }
        if let Some(msg) = cmd.run() {
            let mut reply = Effects::default();
            update(app, msg, &mut reply);
        }
    }
    app.sync_view();
}

#[test]
fn an_embed_with_prose_directly_above_and_below_reaches_live() {
    let (mut app, id) = app_with("prose directly above\n![[image.png]]\nprose directly below\n");
    discover_and_decode(&mut app);

    let doc = app.doc(id).expect("doc");
    let embed = doc
        .embeds
        .images
        .get("image.png")
        .expect("the embed must be tracked even though prose sits on the adjacent lines");
    assert_eq!(
        embed.status,
        ImageStatus::Live,
        "an embed alone on its own line must decode even inside a larger paragraph"
    );
}

/// The isolated layout must keep working — this is the case every other
/// fixture already covers, asserted here so a regression in either direction
/// is caught by one file.
#[test]
fn an_embed_isolated_by_blank_lines_still_reaches_live() {
    let (mut app, id) = app_with("prose\n\n![[image.png]]\n\nprose\n");
    discover_and_decode(&mut app);

    let doc = app.doc(id).expect("doc");
    let embed = doc.embeds.images.get("image.png").expect("embed tracked");
    assert_eq!(embed.status, ImageStatus::Live);
}

/// A truly-inline image — text on the SAME line — must still be left as alt
/// text and never spawned. This is the boundary the per-line rule must not
/// cross.
#[test]
fn an_image_with_text_on_the_same_line_is_not_an_embed() {
    let (mut app, id) = app_with("before ![[image.png]] after\n");
    discover_and_decode(&mut app);

    let doc = app.doc(id).expect("doc");
    assert!(
        doc.embeds.images.is_empty(),
        "an image sharing its line with text must never be spawned"
    );
}
