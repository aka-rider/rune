//! The two chokepoints that post a `ReadOnly` refusal's status message —
//! kept together because both source their wording from `ReadOnly::
//! refusal_message` and both answer the same question ("does this
//! document refuse the action the caller is about to take?"), even though
//! their callers span focus (`focus_title`), rename, save, and close.

use crate::app::App;
use crate::document::DocumentId;
use crate::messages;

/// Why a document refuses mutation — not a plain bool, so a toggleable view
/// mode (`Reading`) can be told apart from a document with no editable form
/// at all (`Always`), and both from a transient, not-yet-committed one
/// (`Preview`): a toggle must not make the Help tab editable, and the
/// undo/redo guard and the `^S` footer hint each branch on the variants
/// differently.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadOnly {
    /// Ordinary editable document.
    No,
    /// The user asked for reading view (⌃P) — the same chord returns it.
    /// The document keeps its journal, its `db` binding and any unsaved
    /// bytes.
    Reading,
    /// No editable form exists: the Help tab, the error banner, an image
    /// document. `commands::reading::toggle` refuses; only a mint site sets
    /// it.
    Always,
    /// A forthcoming Explorer feature previews the file under the cursor
    /// in the Editor without the user having committed to opening it —
    /// this document exists but has not been "opened" in the
    /// ordinary sense. Save, close, and rename all refuse it outright
    /// rather than acting on a document the user never asked to keep; a
    /// later work package flips it to `No` on promotion (the user actually
    /// editing it). Distinct from `Reading`: there is no chord that leaves
    /// `Preview` the way ⌃P leaves `Reading`, so undo/redo join `Reading`
    /// in refusing it rather than following `Always`'s bypass.
    Preview,
}

impl ReadOnly {
    /// The wording for why a read-only document refuses, or `None` for
    /// `No` — which refuses nothing, so it has no wording to give (carry
    /// that out of band instead of a sentinel string a missed check
    /// could pass off as real). `Reading` names the way out because the
    /// user reached it with a chord that also leaves it; `Always` has no
    /// way out to name. The one place both user-initiated refusal
    /// chokepoints (`App::refuse_if_read_only`) source their wording from.
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
