//! The two chokepoints that post a `ReadOnly` refusal's status message —
//! kept together because both source their wording from `ReadOnly::
//! refusal_message` and both answer the same question ("does this
//! document refuse the action the caller is about to take?"), even though
//! their callers span focus (`focus_title`), rename, save, and close.

use crate::app::App;
use crate::document::{DocumentId, ReadOnly};
use crate::messages;

impl App {
    /// The single writer of a read-only refusal's status message: posts
    /// `read_only`'s wording and reports whether it refused — `focus_title`
    /// and `rename::begin` both call this instead of duplicating the check
    /// (`refocus_title`'s silent return on the same precondition is a
    /// re-focus, not a refusal, so it does not).
    pub fn refuse_if_read_only(&mut self, read_only: ReadOnly) -> bool {
        let Some(message) = read_only.refusal_message() else {
            return false;
        };
        messages::warn(self, message);
        true
    }

    /// The narrower sibling of `refuse_if_read_only` above: posts `id`'s
    /// refusal message and returns `true` only when `id` is a not-yet-
    /// committed preview, doing nothing otherwise. `save::trigger_save` and
    /// `workspace::request_close` both call this instead of the generic
    /// check, because that one also refuses `ReadOnly::Reading` — and
    /// unlike rename, both save (^S still materializes bytes already typed
    /// in reading view) and close (closing a reading-view document is
    /// ordinary) must keep working there. A preview has no chord that
    /// leaves it the way ⌃P leaves reading view, so it alone is refused by
    /// either.
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
