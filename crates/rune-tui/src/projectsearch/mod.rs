use crate::app::App;
use crate::pane::Pane;
use crate::runtime::Effects;

pub(crate) mod keys;

pub struct ProjectSearchState {
    pub query: String,
    pub query_generation: crate::generation::ProjectSearchGen,
    pub return_to: crate::returnto::ReturnTo,
}

pub(crate) fn open(app: &mut App, effects: &mut Effects) {
    if app.projectsearch().is_some() {
        return;
    }
    let Some(clearance) = app.clear_title_for_overlay(effects) else {
        return;
    };
    app.close_all_overlays(effects);
    crate::explorer_search::clear_search(app);
    let return_to = crate::returnto::ReturnTo::to(app.active);
    let query_generation = app.next_projectsearch_gen.mint();
    app.open_projectsearch(
        ProjectSearchState {
            query: String::new(),
            query_generation,
            return_to,
        },
        clearance,
    );
    app.set_focus_pane(Pane::Explorer, effects);
}

pub(crate) fn close(app: &mut App) {
    app.close_projectsearch();
}

pub(crate) fn cancel(app: &mut App, effects: &mut Effects) {
    let Some(return_to) = app.projectsearch().map(|s| s.return_to) else {
        return;
    };
    close(app);
    if let Some(target) = return_to.live(app) {
        crate::workspace::switch_to(app, target);
    }
    app.set_focus_pane(Pane::Editor, effects);
}

pub(crate) fn toggle(app: &mut App, effects: &mut Effects) {
    if app.projectsearch().is_some() {
        cancel(app, effects);
    } else {
        open(app, effects);
    }
}

#[cfg(test)]
mod tests;
