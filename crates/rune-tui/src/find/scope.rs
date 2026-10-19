use rune_db::DocId;

use crate::app::App;
use crate::generation::SearchHistoryGen;
use crate::messages;
use crate::runtime::{CmdError, Effects};

pub(crate) const SETTING_KEY: &str = "search_history_scope";
pub(crate) const UNSTORED_NOTICE: &str = "history: this document is not stored yet";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum HistoryScope {
    #[default]
    Doc,
    All,
}

impl HistoryScope {
    fn toggled(self) -> HistoryScope {
        match self {
            HistoryScope::Doc => HistoryScope::All,
            HistoryScope::All => HistoryScope::Doc,
        }
    }

    pub(crate) fn setting_value(self) -> &'static str {
        match self {
            HistoryScope::Doc => "doc",
            HistoryScope::All => "all",
        }
    }

    fn parse(value: &str) -> Option<HistoryScope> {
        [HistoryScope::Doc, HistoryScope::All]
            .into_iter()
            .find(|scope| scope.setting_value() == value)
    }

    fn describe(self) -> &'static str {
        match self {
            HistoryScope::Doc => "this document",
            HistoryScope::All => "all documents",
        }
    }
}

pub(crate) fn origin_doc_id(app: &App) -> Option<DocId> {
    let origin = app.find()?.origin.doc;
    app.doc(origin)?.doc_db().map(|doc_db| DocId(doc_db.db_id))
}

pub(crate) fn unstored(app: &App) -> bool {
    app.db.is_some() && app.history_scope() == HistoryScope::Doc && origin_doc_id(app).is_none()
}

pub(crate) fn request_history(app: &App, generation: SearchHistoryGen, effects: &mut Effects) {
    let Some(db) = app.db.as_ref() else {
        return;
    };
    let doc = match app.history_scope() {
        HistoryScope::All => None,
        HistoryScope::Doc => match origin_doc_id(app) {
            Some(doc_id) => Some(doc_id),
            None => return,
        },
    };
    effects.cmds.push(crate::runtime::load_search_history_cmd(
        db.store.reader_query(),
        generation,
        doc,
    ));
}

fn reload_history(app: &mut App, effects: &mut Effects) {
    let generation = app.next_search_history_gen.mint();
    let Some(state) = app.find_mut() else {
        return;
    };
    state.history_generation = generation;
    state.find.history.clear();
    state.find.leave_history();
    request_history(app, generation, effects);
}

pub(crate) fn toggle(app: &mut App, effects: &mut Effects) {
    let scope = app.history_scope().toggled();
    app.history_scope_choice = Some(scope);
    reload_history(app, effects);
    messages::info(app, format!("search history: {}", scope.describe()));
    let result = app.scope_persistence.touch(app.db.as_ref(), &scope, |db| {
        db.store.set_ui_setting(SETTING_KEY, scope.setting_value())
    });
    if let Some(Err(e)) = result {
        messages::error(app, format!("search history scope not saved: {e}"));
    }
}

pub(crate) fn load_cmd(app: &App, effects: &mut Effects) {
    if let Some(db) = app.db.as_ref() {
        effects.cmds.push(crate::runtime::load_history_scope_cmd(
            db.store.reader_query(),
        ));
    }
}

pub(crate) fn handle_loaded(
    app: &mut App,
    result: Result<Option<String>, CmdError>,
    effects: &mut Effects,
) {
    if app.history_scope_choice.is_some() {
        return;
    }
    let stored = match result {
        Ok(None) => return,
        Ok(Some(value)) => value,
        Err(e) => {
            messages::error(app, format!("search history scope not loaded: {e}"));
            return;
        }
    };
    let Some(scope) = HistoryScope::parse(&stored) else {
        messages::error(
            app,
            format!(
                "stored search history scope {stored:?} is unknown; using {}",
                app.history_scope().describe()
            ),
        );
        return;
    };
    app.history_scope_choice = Some(scope);
    reload_history(app, effects);
}
