//! `ReturnTo` — the document a departed-from excursion (Help, the fuzzy file
//! finder, a browsing session in the Explorer) restores on its way back, if
//! that document is still around to restore to. `App::last_search_query` is
//! deliberately NOT one of these: it holds a query string, not a document
//! to switch back to, and it survives overlay teardown by design rather
//! than being consumed on the way out.

use crate::app::App;
use crate::document::DocumentId;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReturnTo(Option<DocumentId>);

impl ReturnTo {
    pub fn none() -> ReturnTo {
        ReturnTo(None)
    }

    pub fn to(id: DocumentId) -> ReturnTo {
        ReturnTo(Some(id))
    }

    /// The remembered document as recorded, with no liveness check — for a
    /// caller that wants the raw historical fact itself (`navhistory::
    /// departure_origin`), not a live restore target.
    pub fn raw(&self) -> Option<DocumentId> {
        self.0
    }

    /// The remembered document, if it still exists.
    pub fn live(&self, app: &App) -> Option<DocumentId> {
        self.0.filter(|&id| app.documents.contains_key(&id))
    }

    /// [`live`] with one more disqualifier: `excluding` (the document the
    /// caller is departing FROM) is never a valid return target for itself,
    /// even when it still exists.
    pub fn live_excluding(&self, app: &App, excluding: DocumentId) -> Option<DocumentId> {
        self.live(app).filter(|&id| id != excluding)
    }
}
