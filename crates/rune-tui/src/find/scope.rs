use rune_db::DocId;

use crate::app::App;
use crate::document::DocumentId;
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HistoryKey {
    Unrequested,
    All,
    Doc(DocId),
    Unstored,
}

pub(crate) fn stored_id(app: &App, doc: DocumentId) -> Option<DocId> {
    app.doc(doc)?.doc_db().map(|doc_db| DocId(doc_db.db_id))
}

pub(crate) fn wanted_key(app: &App, origin: DocumentId) -> HistoryKey {
    if app.db.is_none() {
        return HistoryKey::Unrequested;
    }
    match app.history_scope() {
        HistoryScope::All => HistoryKey::All,
        HistoryScope::Doc => stored_id(app, origin).map_or(HistoryKey::Unstored, HistoryKey::Doc),
    }
}

pub(crate) fn unstored(app: &App) -> bool {
    app.find()
        .is_some_and(|state| wanted_key(app, state.origin.doc) == HistoryKey::Unstored)
}

pub(crate) fn request_history(app: &App, effects: &mut Effects) {
    let (Some(db), Some(state)) = (app.db.as_ref(), app.find()) else {
        return;
    };
    let doc = match state.history_key {
        HistoryKey::Unrequested | HistoryKey::Unstored => return,
        HistoryKey::All => None,
        HistoryKey::Doc(doc_id) => Some(doc_id),
    };
    effects.cmds.push(crate::runtime::load_search_history_cmd(
        db.store.reader_query(),
        state.history_generation,
        doc,
    ));
}

pub(crate) fn reconcile_history(app: &mut App, effects: &mut Effects) {
    let Some(origin) = app.find().map(|state| state.origin.doc) else {
        return;
    };
    let wanted = wanted_key(app, origin);
    if app.find().is_some_and(|state| state.history_key == wanted) {
        return;
    }
    let generation = app.next_search_history_gen.mint();
    let Some(state) = app.find_mut() else {
        return;
    };
    state.history_generation = generation;
    state.history_key = wanted;
    state.find.history.clear();
    state.find.leave_history();
    request_history(app, effects);
}

pub(crate) fn toggle(app: &mut App, effects: &mut Effects) {
    let scope = app.history_scope().toggled();
    app.history_scope_choice = Some(scope);
    reconcile_history(app, effects);
    messages::info(app, format!("search history: {}", scope.describe()));
    let result = app.scope_persistence.touch(app.db.as_ref(), &scope, |db| {
        db.store.set_ui_setting(SETTING_KEY, scope.setting_value())
    });
    match result {
        Some(Err(e)) => messages::error(app, format!("search history scope not saved: {e}")),
        Some(Ok(_)) => {}
        None => report_unsaved_choice(app),
    }
}

fn report_unsaved_choice(app: &mut App) {
    let reason = match app.db.as_ref() {
        None => "there is no recovery store",
        Some(db) if db.degraded => "the recovery store is degraded",
        Some(_) => return,
    };
    messages::warn(
        app,
        format!("search history scope not saved: {reason}, so the choice lasts until you quit"),
    );
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
    reconcile_history(app, effects);
}
