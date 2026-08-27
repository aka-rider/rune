//! `ensure_loaded` must ask `LayoutMode`, not the raw `Split` flag, before
//! issuing its first `ReadDir`: a column `app.splits.left` still calls
//! shown can be squeezed out of the frame entirely by `layout::geometry`,
//! and a `ReadDir` for a pane nobody can see is a wasted filesystem round
//! trip.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod explorer_common;

use rune_tui::explorer;
use rune_tui::runtime::Effects;

use explorer_common::{app_with, seeded_vfs};

/// A frame too narrow to fit the left column ALONGSIDE the center pane no
/// longer drops the column — `layout::resolve_mode` flips it to a
/// full-width `LayoutMode::ExplorerOnly` instead (the narrow-frame flip),
/// so `ensure_loaded` now DOES request a listing here.
#[test]
fn a_too_narrow_frame_flips_to_explorer_only_and_still_requests_a_listing() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    app.splits.left.show();

    app.frame = Some(rune_tui::app::FrameSize::new(39, 30));

    let mut effects = Effects::default();
    explorer::ensure_loaded(&mut app, &mut effects);

    assert!(
        !effects.cmds.is_empty(),
        "ExplorerOnly paints the Explorer, so ensure_loaded must fill it"
    );
}

/// A frame too SHORT for the column to show anything at all, even at full
/// width, resolves to `LayoutMode::EditorOnly` even though `app.splits.left`
/// still says shown (`layout::resolve_mode`'s own contract) —
/// `ensure_loaded` must honor that and request nothing.
#[test]
fn a_too_short_frame_paints_nothing_and_requests_no_listing() {
    let mem = seeded_vfs();
    let mut app = app_with(&mem);
    app.splits.left.show();
    assert!(app.splits.left.is_shown(), "the raw flag still says shown");

    app.frame = Some(rune_tui::app::FrameSize::new(39, 3));

    let mut effects = Effects::default();
    explorer::ensure_loaded(&mut app, &mut effects);

    assert!(
        effects.cmds.is_empty(),
        "nothing painted this frame, nothing to load"
    );
}
