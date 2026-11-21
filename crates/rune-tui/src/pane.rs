use crate::app::App;
use crate::explorer;
use crate::runtime::Effects;

pub(crate) use crate::pane_command::handle_global_command;
pub(crate) use crate::pane_quit::{handle_quit_key, unpreserved_dirty_docs};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    Explorer,
    Tabs,
    Editor,
    Title,
    Messages,
}

pub(crate) fn show_and_focus_explorer_on_active_file(app: &mut App, effects: &mut Effects) {
    show_explorer_column(app);
    app.set_focus_pane(Pane::Explorer, effects);
    reveal_active_file(app, effects);
}

// The keyboard-free variant: a title that keeps focus keeps it, so the column
// shows and reveals the active file without taking focus.
pub(crate) fn show_explorer_on_active_file(app: &mut App, effects: &mut Effects) {
    show_explorer_column(app);
    reveal_active_file(app, effects);
}

fn show_explorer_column(app: &mut App) {
    app.splits.left.show();
    app.splits.explorer.show();
}

fn reveal_active_file(app: &mut App, effects: &mut Effects) {
    match app.active_doc().resolved_path().cloned() {
        Some(path) => crate::explorer_reveal::reveal(app, &path, effects),
        None => explorer::ensure_loaded(app, effects),
    }
}
