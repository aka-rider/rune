use super::BindMode;
use crate::app::App;
use crate::document::{DocumentId, Hydration};
use crate::messages;
use rune_core::undo::Journal;
use rune_db::{HistoryLoss, LoadResult};

pub(super) struct Restored {
    pub(super) hydration: Option<Hydration>,
    pub(super) mode: BindMode,
}

pub(super) fn restore(
    app: &mut App,
    id: DocumentId,
    load_result: &LoadResult,
    issued_version: Option<u64>,
) -> Option<Restored> {
    let doc = app.doc_mut(id)?;
    let unchanged_since_issue = issued_version == Some(doc.buffer.version());
    let history = &load_result.history;
    if unchanged_since_issue && doc.accepts_history() {
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
    })
}

pub(super) fn report_history(app: &mut App, load_result: &LoadResult, mode: BindMode) {
    if let Some(HistoryLoss::Unreplayable) = load_result.history_loss {
        messages::warn(
            app,
            "earlier undo history could not be replayed \u{2014} starting a fresh one",
        );
    }
    let history = &load_result.history;
    let discarded = !history.done.is_empty() || !history.undone.is_empty();
    if matches!(mode, BindMode::Restarted) && discarded {
        messages::info(
            app,
            "earlier undo history unavailable \u{2014} edited while loading",
        );
    }
}
