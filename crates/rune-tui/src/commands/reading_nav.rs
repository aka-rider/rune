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
        Command::Motion(Motion::MatchBracket, _) => {
            let read_only = app.active_doc().read_only;
            app.refuse_if_read_only(read_only);
            return true;
        }
        _ => {}
    }

    let scrolled = match command {
        Command::Motion(Motion::LineDown, _) => {
            nav_scroll::scroll_lines(app.active_doc_mut(), 1);
            true
        }
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
    let caret = doc.cursors.primary().position.get();
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
