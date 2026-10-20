use super::BindMode;
use crate::app::App;
use crate::document::{DocumentId, Hydration};
use crate::messages;
use rune_core::undo::Journal;
use rune_db::{HistoryLoss, LoadResult};

#[derive(Clone, Copy)]
pub(super) enum Declined {
    EditedWhileLoading,
    FileChangedWhileLoading,
    OpenInAnotherTab,
}

impl Declined {
    fn text(self) -> &'static str {
        match self {
            Declined::EditedWhileLoading => {
                "earlier undo history unavailable \u{2014} edited while loading"
            }
            Declined::FileChangedWhileLoading => {
                "earlier undo history unavailable \u{2014} the file changed while loading"
            }
            Declined::OpenInAnotherTab => {
                "earlier undo history unavailable \u{2014} this file is already open in another tab"
            }
        }
    }
}

pub(super) struct Restored {
    pub(super) hydration: Option<Hydration>,
    pub(super) mode: BindMode,
    pub(super) declined: Option<Declined>,
}

fn seeding_verdict(
    app: &App,
    id: DocumentId,
    load_result: &LoadResult,
    unchanged_since_issue: bool,
) -> Result<(), Declined> {
    let doc = app.doc(id).ok_or(Declined::EditedWhileLoading)?;
    if !unchanged_since_issue || !doc.accepts_history() {
        return Err(Declined::EditedWhileLoading);
    }
    if doc.buffer().content() != load_result.disk_content {
        return Err(Declined::FileChangedWhileLoading);
    }
    if !app.documents_bound_to(load_result.doc_id.0).is_empty() {
        return Err(Declined::OpenInAnotherTab);
    }
    Ok(())
}

pub(super) fn restore(
    app: &mut App,
    id: DocumentId,
    load_result: &LoadResult,
    issued_version: Option<u64>,
) -> Option<Restored> {
    let unchanged_since_issue = issued_version == Some(app.doc(id)?.buffer().version());
    let verdict = seeding_verdict(app, id, load_result, unchanged_since_issue);
    let doc = app.doc_mut(id)?;
    let history = &load_result.history;
    if verdict.is_ok() {
        let journal = Journal::from_parts(
            history.done.iter().map(|s| s.step.clone()).collect(),
            history.undone.iter().map(|s| s.step.clone()).collect(),
        );
        let hydration = doc.restore_lineage(
            &load_result.disk_content,
            &load_result.recovered.content,
            &load_result.recovered.cursors,
            journal,
        );
        return Some(Restored {
            hydration: Some(hydration),
            mode: BindMode::Seeded {
                pos: history.done.len(),
            },
            declined: None,
        });
    }
    let hydration = unchanged_since_issue.then(|| {
        doc.hydrate(
            &load_result.disk_content,
            &load_result.recovered.content,
            &load_result.recovered.cursors,
        )
    });
    Some(Restored {
        hydration,
        mode: BindMode::Restarted,
        declined: verdict.err(),
    })
}

pub(super) fn report_history(app: &mut App, load_result: &LoadResult, declined: Option<Declined>) {
    for loss in &load_result.history_losses {
        match loss {
            HistoryLoss::Unreplayable => messages::warn(
                app,
                "earlier undo history could not be replayed \u{2014} undo starts here",
            ),
            HistoryLoss::Unreadable => messages::warn(
                app,
                "earlier undo history could not be read \u{2014} undo starts here",
            ),
            HistoryLoss::PositionRepaired => messages::warn(
                app,
                "the stored undo position was out of range \u{2014} showing the latest text",
            ),
            HistoryLoss::RedoTailDropped => messages::info(
                app,
                "the file changed on disk \u{2014} earlier redo steps no longer apply",
            ),
        }
    }
    let history = &load_result.history;
    let discarded = !history.done.is_empty() || !history.undone.is_empty();
    if let Some(reason) = declined
        && discarded
    {
        messages::info(app, reason.text());
    }
}
