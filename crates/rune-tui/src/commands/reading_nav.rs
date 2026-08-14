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

use rune_core::coords::{DisplayRow, WrapPoint};
use rune_core::cursor::CursorSet;
use rune_nav::{Ref, RefKind, UseRole};
use rune_syntax::element::ByteRange;

use crate::app::App;
use crate::commands::nav_scroll;
use crate::document::Document;
use crate::keymap::{Command, Motion};
use crate::viewport::ScrollMode;

fn pin_caret_to_first_visible_line(doc: &mut Document) {
    let view = doc.view();
    let wrap_row = view.display.display_to_wrap(doc.viewport.scroll_row);
    let syntax_point = view.wrap.wrap_to_syntax(
        doc.buffer.content(),
        WrapPoint {
            row: wrap_row.0,
            col: 0,
        },
    );
    let buffer_point = view.syntax.syntax_to_buffer(syntax_point);
    let offset = doc.buffer.line_col_to_offset(buffer_point);
    doc.cursors = CursorSet::new(offset);
}

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
        Command::Indent => {
            focus_link(app, LinkStep::Next);
            return true;
        }
        Command::Outdent => {
            focus_link(app, LinkStep::Prev);
            return true;
        }
        _ => {}
    }

    let scrolled = match command {
        Command::Motion(Motion::LineDown, _) => {
            nav_scroll::scroll_lines(app.active_doc_mut(), 1);
            true
        }
        // Up at the very top of the viewport re-keys the existing
        // buffer-top-focuses-title gesture (`dispatch::at_view_top`) to the
        // read-only document's own "top" — the first visible row, since a
        // read-only document's cursor position carries no visible meaning
        // to test against.
        Command::Motion(Motion::LineUp, _) => {
            if app.active_doc().viewport.scroll_row == DisplayRow(0) {
                app.focus_title();
                false
            } else {
                nav_scroll::scroll_lines(app.active_doc_mut(), -1);
                true
            }
        }
        Command::Motion(Motion::CharLeft, _)
        | Command::Motion(Motion::WordLeft, _)
        | Command::Motion(Motion::PageUp, _) => {
            let step = nav_scroll::page_step(app.active_doc());
            nav_scroll::scroll_lines(app.active_doc_mut(), -step);
            true
        }
        Command::Motion(Motion::CharRight, _)
        | Command::Motion(Motion::WordRight, _)
        | Command::Motion(Motion::PageDown, _) => {
            let step = nav_scroll::page_step(app.active_doc());
            nav_scroll::scroll_lines(app.active_doc_mut(), step);
            true
        }
        Command::Motion(Motion::LineStart, _) => {
            nav_scroll::scroll_to_document_top(app.active_doc_mut());
            true
        }
        Command::Motion(Motion::LineEnd, _) => {
            nav_scroll::scroll_to_document_bottom(app.active_doc_mut());
            true
        }
        _ => return false,
    };
    if scrolled {
        pin_caret_to_first_visible_line(app.active_doc_mut());
    }
    true
}

#[derive(Clone, Copy)]
enum LinkStep {
    Next,
    Prev,
}

fn link_sites(catalogue: &[Ref]) -> impl Iterator<Item = ByteRange> + '_ {
    catalogue.iter().filter_map(|r| match &r.kind {
        RefKind::Use {
            role: UseRole::Link,
            ..
        } => Some(r.site),
        _ => None,
    })
}

fn focus_link(app: &mut App, step: LinkStep) {
    let doc = app.active_doc();
    let bound = doc.reading_link_focus.map(|site| site.start);
    let caret = doc.cursors.primary().position;
    let sites: Vec<ByteRange> = link_sites(&doc.catalogue).collect();

    let target = match step {
        LinkStep::Next => {
            let after = bound.unwrap_or(caret);
            let inclusive = bound.is_none();
            sites
                .iter()
                .find(|site| {
                    if inclusive {
                        site.start >= after
                    } else {
                        site.start > after
                    }
                })
                .or_else(|| sites.first())
        }
        LinkStep::Prev => {
            let before = bound.unwrap_or(caret);
            sites
                .iter()
                .rev()
                .find(|site| site.start < before)
                .or_else(|| sites.last())
        }
    };
    let Some(&site) = target else { return };

    let doc = app.active_doc_mut();
    doc.reading_link_focus = Some(site);
    doc.cursors = CursorSet::new(site.start);
    doc.viewport.mode = ScrollMode::EnsureVisible;
}
