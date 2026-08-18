//! `pane::handle_global_command`'s own per-`GlobalCommand` arm bodies, split
//! out to keep `pane.rs` under the 500-line budget — the match in
//! `handle_global_command` itself stays a routing table into these.

use crate::app::App;
use crate::messages;
use crate::pane::{Pane, show_and_focus_explorer_on_active_file};
use crate::runtime::Effects;

/// `GlobalCommand::ToggleLeft` — painted this frame ⇒ hide it and hand
/// focus to the Editor; not painted ⇒ show it, focus the Explorer, and
/// land the cursor on the active document's own file. `painted_before` is
/// `handle_global_command`'s own `layout_mode()` snapshot, taken before
/// this press's own effects.
pub(crate) fn toggle_left(app: &mut App, painted_before: bool, effects: &mut Effects) {
    if painted_before {
        app.splits.left.hide();
        crate::focus::reconcile(app, effects);
    } else {
        show_and_focus_explorer_on_active_file(app, effects);
    }
}

/// `GlobalCommand::FocusTabs` — mirrors `FocusExplorer`'s "show + focus"
/// pairing: the Tabs pane's own cursor is meaningless to a user who can't
/// see it. Also raises a starved split from a dragged-down divider back to
/// its floor before focus lands there.
pub(crate) fn focus_tabs(app: &mut App, effects: &mut Effects) {
    app.splits.left.show();
    let area = ratatui::layout::Rect::new(0, 0, app.frame_width, app.frame_height);
    let geo = crate::layout::geometry(area, app);
    if let Some(block) = geo.left_block {
        let budget = crate::layout::explorer_budget(block);
        app.splits
            .explorer
            .ensure_trail(budget, crate::layout::TABS_LIMITS);
    }
    app.set_focus_pane(Pane::Tabs, effects);
}

/// `GlobalCommand::Help` — mints/toggles the generated Help virtual
/// document. `handle_global_command`'s hoisted blur gate fires only for
/// `Pane::Title`, so this moves focus to the Editor itself too — without
/// that, `F1` pressed from the Explorer or Tabs pane would switch the
/// active document while focus stayed stranded on the chrome list.
pub(crate) fn help(app: &mut App, effects: &mut Effects) {
    app.set_focus_pane(Pane::Editor, effects);
    crate::workspace::toggle_help(app, effects);
}

/// `GlobalCommand::NewDocument` — same pre-switch focus move as `help`
/// above, and for the same reason.
pub(crate) fn new_document(app: &mut App, effects: &mut Effects) {
    app.set_focus_pane(Pane::Editor, effects);
    let departed = crate::navhistory::departure_origin(app);
    crate::workspace::new_untitled_document(app);
    crate::navhistory::record_departure_if_moved(app, departed);
    app.focus_title();
}

/// `GlobalCommand::TabSwitch` — out-of-range is a silent no-op, so a digit
/// naming a tab that isn't open does nothing rather than guessing at a
/// neighbour. Same pre-switch focus move as `help` above.
pub(crate) fn tab_switch(app: &mut App, idx: usize, effects: &mut Effects) {
    app.set_focus_pane(Pane::Editor, effects);
    crate::workspace::select_tab(app, idx);
}

/// `GlobalCommand::Trash` — the finder is never trashed out from under:
/// while it's open the active document may just be a file the user arrowed
/// past, never opened for real (decision: Trash behaves like Esc there
/// instead).
pub(crate) fn trash(app: &mut App, effects: &mut Effects) {
    if app.filesearch().is_some() {
        crate::filesearch::cancel(app, effects);
        messages::info(
            app,
            "file finder closed \u{2014} open the file before trashing it",
        );
    } else {
        crate::trash::request_trash(app, effects);
    }
}

/// `GlobalCommand::ToggleSearch` — open creates a fresh, focused, empty
/// draft; close saves it as `App::last_search_query` and clears the
/// highlight overlay (`search::open`/`close`, the chokepoints — neither
/// touches `App::focus`, since the bar was never a `Pane`). Refused while a
/// merge is active on the active document — same precondition
/// `focus_title` already refuses on — since the resolver claims every
/// editor key for itself and a bar opening on top of it would steal keys
/// the resolver never gets a chance to see.
pub(crate) fn toggle_search(app: &mut App, effects: &mut Effects) {
    if app.search().is_some() {
        crate::search::close(app);
    } else if matches!(app.merge, crate::merge::MergeState::Active { doc, .. } if doc == app.active)
    {
        messages::info(app, "finish the merge first (^M)");
    } else {
        crate::search::open(app);
        // Kicks off the ONE history load this bar-open needs, off-thread
        // through a cloned `ReaderQuery` — never gated on `Db::degraded` (a
        // write-path flag; reads run on their own connection, unaffected
        // by it). No store at all (the extreme construction-failure
        // fallback) just leaves history empty, same as an ordinary reader
        // failure.
        if let (Some(db), Some(generation)) =
            (app.db.as_ref(), app.search().map(|s| s.history_generation))
        {
            effects.cmds.push(crate::runtime::load_search_history_cmd(
                db.store.reader_query(),
                generation,
            ));
        }
    }
}

/// `GlobalCommand::ToggleFileSearch` — active -> `cancel` (closes it,
/// restores the document that was active before it opened, focuses the
/// Editor); inactive -> `open`.
pub(crate) fn toggle_file_search(app: &mut App, effects: &mut Effects) {
    if app.filesearch().is_some() {
        crate::filesearch::cancel(app, effects);
    } else {
        crate::filesearch::open(app, effects);
    }
}

pub(crate) fn toggle_palette(app: &mut App, effects: &mut Effects) {
    if app.palette().is_some() {
        crate::palette::close(app);
    } else {
        crate::palette::open(app, effects);
    }
}
