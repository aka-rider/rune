//! `Pane` — the focus discriminant only, no trait objects; `Explorer`/
//! `Tabs`'s own state lands in plain named `App` fields. Extracted out of
//! `app.rs` to keep it under the 500-line budget. The `GlobalCommand`
//! handler match lives in `pane_command.rs`, the quit-confirm state machine
//! in `pane_quit.rs`, the bar-close policy table in `pane_bar_policy.rs`,
//! and the registry refusal check in `pane_refusal.rs` — re-exported here
//! so `pane::` call sites outside this module keep working unchanged.

use crate::app::App;
use crate::explorer;
use crate::runtime::Effects;

pub(crate) use crate::pane_command::handle_global_command;
pub(crate) use crate::pane_quit::{handle_quit_key, unpreserved_dirty_docs};

/// Which chrome region owns the next keystroke once the global table
/// (`keymap::GLOBAL_BINDINGS`) doesn't claim it — the pane-routing stage
/// of the key pipeline, after the global table and before pane-local keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    Explorer,
    Tabs,
    Editor,
    /// The editable title field (`title.rs`) — focused by `^r` or by
    /// pressing Up at the top of the editor. While it owns focus every
    /// keystroke goes to the file name and none of them reach the buffer.
    Title,
    /// The collapsible message-log pane above the footer — focused by
    /// `^E`/`⌘E` while the pane is open, or by clicking inside it. Only
    /// ever focusable while `messages::is_open` is true
    /// (`LayoutMode::focusable`'s own gate).
    Messages,
}

/// Shows the left column, focuses the Explorer, and lands the cursor on the
/// active document's own file — the ONE chokepoint both `ToggleLeft`'s show
/// branch (above) and the editor's Escape cascade (`dispatch::
/// handle_editor_key`) reach through, so the two triggers can never drift
/// apart on how "reveal the active file" behaves. A document with no
/// `file_path` (a draft, or the virtual Help document) has nothing to
/// reveal, so it falls back to the Explorer's ordinary first-load fill
/// instead of calling `explorer_reveal::reveal`.
pub(crate) fn show_and_focus_explorer_on_active_file(app: &mut App, effects: &mut Effects) {
    app.splits.left.show();
    app.splits.explorer.show();
    app.set_focus_pane(Pane::Explorer, effects);
    match app.active_doc().file_path.clone() {
        Some(path) => crate::explorer_reveal::reveal(app, &path, effects),
        None => explorer::ensure_loaded(app, effects),
    }
}
