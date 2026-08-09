//! The whole "read-only means viewport" policy for a document with no
//! insertion point (`Document::has_insertion_point`): the Help tab, an image
//! document, the error banner, and an ordinary document in `⌃P` reading
//! view. None of these paints a caret, so a cursor-driven motion command
//! (`commands::nav`/`nav_line`/`nav_scroll`'s cursor-moving half) is
//! invisible until the cursor wanders far enough for
//! `Viewport::reconcile`'s scrolloff band to finally chase it — the House
//! Rule violation this module closes: every motion key must move the screen
//! on its very first press.
//!
//! `intercept` is the one place that branches on read-only-ness for
//! movement, so `nav`/`nav_line`/`nav_scroll` themselves stay exactly as
//! they were (cursor-driven, for the editable case they were written for),
//! and `dispatch::handle_editor_key`'s big `match` keeps dispatching
//! commands to those same handlers unchanged for every editable document.
//!
//! Document-reader convention (Preview, Evince, iBooks): Left/PageUp and
//! Right/PageDown page; Home/End jump to the first/last page; a Shift
//! chord scrolls exactly like its bare key, because keyboard selection does
//! not exist in a read-only document at all.

use crate::app::App;
use crate::commands::nav_scroll;
use crate::keymap::Command;

/// Returns true when `command` was handled as a viewport command on the
/// active document — the caller must treat that as fully consumed and stop,
/// exactly like `KeyOutcome::Consumed`. Returns `false` immediately unless
/// the active document is read-only, and for any command outside the
/// motion set below: an edit command on a read-only document must still
/// fall through to `commands::edit_core`'s `refuse_if_read_only`
/// chokepoint, untouched by this module.
pub fn intercept(app: &mut App, command: Command) -> bool {
    if !app.active_doc().is_read_only() {
        return false;
    }

    match command {
        Command::LineDown | Command::SelectLineDown => {
            nav_scroll::scroll_lines(app.active_doc_mut(), 1);
        }
        // Up at the very top of the viewport re-keys the existing
        // buffer-top-focuses-title gesture (`dispatch::at_view_top`) to the
        // read-only document's own "top" — the first visible row, since a
        // read-only document's cursor position carries no visible meaning
        // to test against.
        Command::LineUp | Command::SelectLineUp => {
            if app.active_doc().viewport.scroll_row == 0 {
                app.focus_title();
            } else {
                nav_scroll::scroll_lines(app.active_doc_mut(), -1);
            }
        }
        Command::CharLeft
        | Command::WordLeft
        | Command::SelectCharLeft
        | Command::SelectWordLeft
        | Command::PageUp
        | Command::SelectPageUp => {
            let step = nav_scroll::page_step(app.active_doc());
            nav_scroll::scroll_lines(app.active_doc_mut(), -step);
        }
        Command::CharRight
        | Command::WordRight
        | Command::SelectCharRight
        | Command::SelectWordRight
        | Command::PageDown
        | Command::SelectPageDown => {
            let step = nav_scroll::page_step(app.active_doc());
            nav_scroll::scroll_lines(app.active_doc_mut(), step);
        }
        Command::LineStart | Command::SelectLineStart => {
            nav_scroll::scroll_to_document_top(app.active_doc_mut());
        }
        Command::LineEnd | Command::SelectLineEnd => {
            nav_scroll::scroll_to_document_bottom(app.active_doc_mut());
        }
        _ => return false,
    }
    true
}
