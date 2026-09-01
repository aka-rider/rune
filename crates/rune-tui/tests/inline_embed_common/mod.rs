//! Shared setup for the inline-embed test files (`inline_embed.rs` and
//! `inline_embed_resize.rs`) — a markdown document with one embedded image,
//! discovered and decoded through the real update loop, so a test states
//! only what it's actually asserting.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_core::cursor::CursorSet;
use rune_tui::app::{App, update};
use rune_tui::document::DocumentId;
use rune_tui::runtime::{CmdKind, Effects, Msg};
use rune_vfs::{Mem, Vfs, VfsTestExt};

pub const X_PNG: &[u8] = include_bytes!("../../../../testdata/assets/x.png");

/// A markdown document bound to `/vault/doc.md`, with `x.png` seeded
/// alongside it in the same `Mem` vfs, and the buffer parsed once (plan
/// gotcha: `sync_embeds` reads `doc.doc.blocks()`, populated only by a
/// prior `view()`/`sync_view()` call — the real `runtime::run` bootstrap
/// always calls `sync_view` once before the first message, so tests must
/// mirror that instead of sending the first message against an unparsed
/// document).
pub fn app_with_embed(content: &str) -> (App, DocumentId) {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(Path::new("/vault/x.png"), X_PNG)
        .expect("seed x.png");
    let vfs: Arc<dyn Vfs + Send + Sync> = mem;
    let mut app = App::new(
        Buffer::new(content),
        Some(
            rune_tui::resolved::ResolvedPath::resolve(
                vfs.as_ref(),
                std::path::Path::new(&Path::new("/vault/doc.md").to_path_buf()),
            )
            .expect("the launch path resolves"),
        ),
        vfs,
        None,
    );
    let id = app.active;
    app.graphics.kitty = true;
    // A real session always has a resolved workspace root
    // (`workspaceroot::resolve`, run at startup); `rune_nav::resolve`'s
    // relative-candidate containment check requires a non-empty one.
    app.set_root(Path::new("/vault").to_path_buf());
    // `Document::new`'s default cursor sits at buffer offset 0 — which,
    // for content starting with the image markup itself, is INSIDE the
    // image's own byte range and would reveal it (RevealGrant::Decide)
    // before `sync_view` below even runs once, defeating
    // `standalone_image`'s `Rendered`-only requirement from the very
    // first reconcile pass. Move it to the buffer's end (safely outside
    // any single-line fixture's image range) so every test starts from a
    // genuinely Rendered, spawnable embed; a test that specifically wants
    // the caret ON the image moves it there itself, afterwards.
    let end = content.len();
    app.doc_mut(id).expect("doc").cursors = CursorSet::new(end);
    app.sync_view();
    (app, id)
}

/// Runs every `CmdKind::ImageDecode` `Cmd` `effects` collected synchronously
/// and feeds each reply straight back through `update` — generalised to
/// however many embeds a single reconcile pass spawned.
fn run_decodes(app: &mut App, effects: Effects) {
    for cmd in effects.cmds {
        if !matches!(cmd.kind(), CmdKind::ImageDecode | CmdKind::ImageEncode) {
            continue;
        }
        if let Some(msg) = cmd.run() {
            let mut reply_effects = Effects::default();
            update(app, msg, &mut reply_effects);
            run_decodes(app, reply_effects);
        }
    }
}

/// Sends a harmless message (a resize to a real pane size) so the post-
/// dispatch chokepoint (`dispatch::after_update`) runs `sync_embeds` once,
/// then decodes whatever it spawned.
pub fn discover_and_decode(app: &mut App) {
    let mut effects = Effects::default();
    update(app, Msg::Resize(60, 20), &mut effects);
    run_decodes(app, effects);
    app.sync_view();
}
