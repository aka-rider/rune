//! Regression: an inline embed's cell footprint used to stay stale across
//! a pane resize until an mtime change forced a re-decode — only the
//! whole-document image path re-fit. Split out of `inline_embed.rs` to
//! keep that file under the repo's 500-line ceiling; shares its setup via
//! `inline_embed_common`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use rune_tui::app::{App, update};
use rune_tui::document::DocumentId;
use rune_tui::graphics::ImageStatus;
use rune_tui::runtime::{CmdKind, Effects, Msg};

mod inline_embed_common;
use inline_embed_common::{app_with_embed, discover_and_decode};

fn embed_cells(app: &App, id: DocumentId) -> Option<rune_image::CellFootprint> {
    match &app.doc(id)?.embeds()?.images.get("x.png")?.status {
        ImageStatus::Live { cells, .. } => Some(*cells),
        _ => None,
    }
}

/// Shrinking the pane below the embed's natively-fitting width must re-fit
/// it and re-transmit, exactly like the whole-document image path already
/// does on a resize.
#[test]
fn shrinking_the_pane_refits_an_inline_embeds_footprint() {
    let (mut app, id) = app_with_embed("![caption](x.png)\n");
    discover_and_decode(&mut app);
    let before = embed_cells(&app, id).expect("test setup: embed must be Live");
    assert_eq!(
        before,
        rune_image::CellFootprint { cols: 8, rows: 3 },
        "test setup: x.png (64x48px, 8x16 cells) fits in 8 cols at this width"
    );

    let mut effects = Effects::default();
    update(&mut app, Msg::Resize(8, 20), &mut effects);

    let after = embed_cells(&app, id).expect("the embed must still be Live");
    assert_ne!(
        before, after,
        "the embed's footprint must actually change once the pane narrows past it"
    );
    assert!(
        effects
            .cmds
            .iter()
            .any(|c| c.kind() == CmdKind::ImageEncode),
        "a changed embed footprint must re-encode and retransmit, not just update the stored cells"
    );
}
