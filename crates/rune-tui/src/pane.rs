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
    app.splits.left.show();
    app.splits.explorer.show();
    app.set_focus_pane(Pane::Explorer, effects);
    match app.active_doc().file_path.clone() {
        Some(path) => crate::explorer_reveal::reveal(app, &path, effects),
        None => explorer::ensure_loaded(app, effects),
    }
}
