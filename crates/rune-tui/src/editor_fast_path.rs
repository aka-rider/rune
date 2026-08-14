//! The hardcoded Enter/Escape fast path `dispatch::handle_editor_key`
//! checks before `keymap::resolve` — split out to keep `dispatch.rs` under
//! the 500-line budget.

use crate::app::App;
use crate::commands::{edit, nav};
use crate::keymap::{KeyCode, KeyInput, Mods};
use crate::navigate;
use crate::pane;
use crate::runtime::Effects;

/// Enter (mod 0) -> newline; Escape -> collapse selection. Neither is a
/// resolver-bound chord (plan Context, "Keymap") — checked before
/// `keymap::resolve` and the printable-insert fallthrough. Returns whether
/// `key` was one of these two and has already been fully handled.
pub(crate) fn hardcoded_fast_path(app: &mut App, key: KeyInput, effects: &mut Effects) -> bool {
    if key.code == KeyCode::Enter && key.mods == Mods::NONE {
        if app.active_doc().is_read_only() {
            navigate::follow(app, effects);
        } else {
            edit::newline(app, app.active);
        }
        return true;
    }
    if key.code == KeyCode::Escape && key.mods == Mods::NONE {
        // The cascade — multi-cursor, then selection, then leave to the
        // Explorer: `nav::escape` collapses whichever of the first two it
        // finds and reports `Unconsumed` only once neither applies, which
        // is this fast path's own cue to hand focus to the Explorer instead
        // — unfolding the left column if it's collapsed, and landing the
        // cursor on the active document's file (`pane::
        // show_and_focus_explorer_on_active_file`, shared with `^B`'s show
        // branch).
        if nav::escape(app.active_doc_mut()) == nav::EscapeOutcome::Unconsumed {
            pane::show_and_focus_explorer_on_active_file(app, effects);
        }
        return true;
    }
    false
}
