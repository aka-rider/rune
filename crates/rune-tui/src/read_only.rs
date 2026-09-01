use crate::app::App;
use crate::messages;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadOnly {
    No,
    /// The user asked for reading view (⌃⇧P) — the same chord returns it,
    /// with the document's journal, `db` binding, and unsaved bytes intact.
    Reading,
    /// No editable form exists: the Help tab, the error banner, an image
    /// document. Nothing can toggle it back — only a mint site sets it.
    Always,
}

impl ReadOnly {
    pub fn refusal_message(&self) -> Option<&'static str> {
        match self {
            ReadOnly::No => None,
            ReadOnly::Reading => Some("reading view — ⌃⇧P to edit"),
            ReadOnly::Always => Some("this document is read-only"),
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
}
