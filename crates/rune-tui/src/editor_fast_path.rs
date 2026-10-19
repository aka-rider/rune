use crate::app::App;
use crate::commands::{edit, nav};
use crate::keymap::{KeyCode, KeyInput, Mods};
use crate::navigate;
use crate::pane;
use crate::runtime::Effects;

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
        // `nav::escape` collapses a multi-cursor down to one, or failing
        // that a single selection down to a caret; `Unconsumed` means
        // neither applied, which hands focus to the Explorer instead.
        if nav::escape(app.active_doc_mut()) == nav::EscapeOutcome::Unconsumed {
            pane::show_and_focus_explorer_on_active_file(app, effects);
        }
        return true;
    }
    false
}
