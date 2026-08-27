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

    pub fn raw(&self) -> Option<DocumentId> {
        self.0
    }

    pub fn live(&self, app: &App) -> Option<DocumentId> {
        self.0.filter(|&id| app.documents.contains_key(&id))
    }

    pub fn live_excluding(&self, app: &App, excluding: DocumentId) -> Option<DocumentId> {
        self.live(app).filter(|&id| id != excluding)
    }
}
