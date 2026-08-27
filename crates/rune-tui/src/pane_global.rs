use crate::app::App;
use crate::messages;
use crate::pane::{Pane, show_and_focus_explorer_on_active_file};
use crate::runtime::Effects;

pub(crate) fn toggle_left(app: &mut App, painted_before: bool, effects: &mut Effects) {
    if painted_before {
        app.splits.left.hide();
        crate::focus::reconcile(app, effects);
    } else {
        show_and_focus_explorer_on_active_file(app, effects);
    }
}

pub(crate) fn focus_tabs(app: &mut App, effects: &mut Effects) {
    app.splits.left.show();
    let area = app.frame_area();
    let geo = crate::layout::geometry(area, app);
    if let Some(block) = geo.left_block {
        let budget = crate::layout::explorer_budget(block);
        app.splits
            .explorer
            .ensure_trail(budget, crate::layout::TABS_LIMITS);
    }
    app.set_focus_pane(Pane::Tabs, effects);
}

// `handle_global_command`'s hoisted blur gate fires only for `Pane::Title`,
// so `F1` pressed from the Explorer or Tabs pane needs its own focus move to
// the Editor — otherwise the active document would switch while focus
// stayed stranded on the chrome list.
pub(crate) fn help(app: &mut App, effects: &mut Effects) {
    app.set_focus_pane(Pane::Editor, effects);
    crate::workspace::toggle_help(app, effects);
}

pub(crate) fn new_document(app: &mut App, effects: &mut Effects) {
    app.set_focus_pane(Pane::Editor, effects);
    let departed = crate::navhistory::departure_origin(app);
    crate::workspace::new_untitled_document(app);
    crate::navhistory::record_departure_if_moved(app, departed);
    app.focus_title();
}

pub(crate) fn tab_switch(app: &mut App, idx: usize, effects: &mut Effects) {
    if !crate::workspace::select_tab(app, idx) {
        messages::warn(app, "no tab at that number");
        return;
    }
    app.set_focus_pane(Pane::Editor, effects);
}

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

pub(crate) fn toggle_search(app: &mut App, effects: &mut Effects) {
    if app.search().is_some() {
        crate::search::close(app);
    } else if matches!(app.merge, crate::merge::MergeState::Active { doc, .. } if doc == app.active)
    {
        messages::info(app, "finish the merge first (^M)");
    } else {
        crate::search::open(app, effects);
        // Never gated on `Db::degraded` — that's a write-path flag; reads
        // run on their own connection, unaffected by it. No store at all
        // just leaves history empty, same as an ordinary reader failure.
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::keymap::GlobalCommand;
    use crate::messages;
    use crate::pane_command::handle_global_command;
    use rune_core::buffer::Buffer;
    use rune_vfs::Mem;
    use std::sync::Arc;

    fn app() -> App {
        App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None)
    }

    #[test]
    fn tab_switch_out_of_range_leaves_focus_untouched_and_warns() {
        let mut app = app();
        app.frame = Some(crate::app::FrameSize::new(120, 34));
        app.splits.left.show();
        app.set_focus_pane(Pane::Tabs, &mut Effects::default());
        let active_before = app.active;

        let mut effects = Effects::default();
        handle_global_command(&mut app, GlobalCommand::TabSwitch(5), &mut effects);

        assert_eq!(
            app.focus(),
            Pane::Tabs,
            "an out-of-range tab switch must never move focus"
        );
        assert_eq!(
            app.active, active_before,
            "the active document must not change"
        );
        assert_eq!(messages::newest_text(&app), Some("no tab at that number"));
    }

    #[test]
    fn tab_switch_in_range_activates_the_tab_and_focuses_the_editor() {
        let mut app = app();
        app.frame = Some(crate::app::FrameSize::new(120, 34));
        let second = app.open_document(Buffer::new("second"));
        app.set_focus_pane(Pane::Tabs, &mut Effects::default());

        let mut effects = Effects::default();
        handle_global_command(&mut app, GlobalCommand::TabSwitch(1), &mut effects);

        assert_eq!(app.active, second);
        assert_eq!(app.focus(), Pane::Editor);
        assert_eq!(messages::newest_text(&app), None);
    }

    #[test]
    fn ctrl_e_with_an_invalid_title_posts_one_refusal_and_leaves_the_title_focused() {
        let mut app = app();
        app.focus_title();
        assert_eq!(
            app.focus(),
            Pane::Title,
            "test setup: title must be focused"
        );
        app.title.set_text("bad/name");
        let posts_before = messages::posts(&app);

        let mut effects = Effects::default();
        handle_global_command(&mut app, GlobalCommand::ToggleMessages, &mut effects);

        assert_eq!(
            app.focus(),
            Pane::Title,
            "an invalid title must keep the keyboard"
        );
        assert!(
            !crate::messages::doc(&app).focused,
            "the messages pane must not paint itself focused while the title holds the keyboard"
        );
        assert_eq!(
            messages::posts(&app),
            posts_before + 1,
            "the refusal must post exactly once, not once per blur attempt"
        );
        assert_eq!(
            messages::newest_text(&app),
            Some("that name can't be used for a file")
        );
    }
}
