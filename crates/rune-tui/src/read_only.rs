use crate::app::App;
use crate::document::DocumentId;
use crate::messages;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadOnly {
    No,
    /// The user asked for reading view (⌃P) — the same chord returns it,
    /// with the document's journal, `db` binding, and unsaved bytes intact.
    Reading,
    /// No editable form exists: the Help tab, the error banner, an image
    /// document. Nothing can toggle it back — only a mint site sets it.
    Always,
    /// The Explorer previews the file under the cursor in the Editor
    /// without the user having committed to opening it — this document
    /// exists but has not been "opened" in the ordinary sense. A later
    /// promotion flips it to `No` once the user actually edits it.
    Preview,
}

impl ReadOnly {
    pub fn refusal_message(&self) -> Option<&'static str> {
        match self {
            ReadOnly::No => None,
            ReadOnly::Reading => Some("reading view — ⌃P to edit"),
            ReadOnly::Always => Some("this document is read-only"),
            ReadOnly::Preview => Some("preview — not yet open for editing"),
        }
    }
}

impl App {
    pub fn refuse_if_read_only(&mut self, read_only: ReadOnly) -> bool {
        let Some(message) = read_only.refusal_message() else {
            return false;
        };
        messages::warn_if_new(self, message);
        true
    }

    /// Narrower than `refuse_if_read_only`: refuses only a not-yet-
    /// committed preview, leaving reading view untouched — `^S` must still
    /// materialize bytes typed in reading view, and closing it is ordinary.
    pub fn refuse_if_preview(&mut self, id: DocumentId) -> bool {
        if !self
            .doc(id)
            .is_some_and(crate::document::Document::is_preview)
        {
            return false;
        }
        if let Some(message) = ReadOnly::Preview.refusal_message() {
            messages::warn(self, message);
        }
        true
    }
}
