use crate::app::App;
use crate::commands::{
    clipboard, edit, edit_lines, edit_lines_move, multi, nav, nav_line, nav_scroll, reading_nav,
};
use crate::keymap::{self, Command, Extend, Motion, QuitKey};
use crate::navigate;
use crate::pane;
use crate::runtime::{Effects, PasteTarget};
use crate::save;

pub(crate) fn run(
    app: &mut App,
    command: Command,
    quit_key: Option<QuitKey>,
    effects: &mut Effects,
) -> keymap::KeyOutcome {
    if reading_nav::intercept(app, command) {
        return keymap::KeyOutcome::Consumed;
    }

    match command {
        Command::Motion(Motion::CharLeft, extend) => nav::char_left(app.active_doc_mut(), extend),
        Command::Motion(Motion::CharRight, extend) => nav::char_right(app.active_doc_mut(), extend),
        Command::Motion(Motion::LineUp, Extend::No) => {
            if at_buffer_top(app) {
                app.focus_title();
            } else {
                nav_scroll::line_up(app.active_doc_mut(), Extend::No);
            }
        }
        Command::Motion(Motion::LineUp, Extend::Yes) => {
            nav_scroll::line_up(app.active_doc_mut(), Extend::Yes);
        }
        Command::Motion(Motion::LineDown, extend) => {
            nav_scroll::line_down(app.active_doc_mut(), extend)
        }
        Command::Motion(Motion::WordLeft, extend) => nav::word_left(app.active_doc_mut(), extend),
        Command::Motion(Motion::WordRight, extend) => nav::word_right(app.active_doc_mut(), extend),
        Command::Motion(Motion::LineStart, extend) => {
            nav_line::line_start(app.active_doc_mut(), extend)
        }
        Command::Motion(Motion::LineEnd, extend) => {
            nav_line::line_end(app.active_doc_mut(), extend)
        }
        Command::Motion(Motion::PageUp, extend) => {
            nav_scroll::page_up(app.active_doc_mut(), extend)
        }
        Command::Motion(Motion::PageDown, extend) => {
            nav_scroll::page_down(app.active_doc_mut(), extend)
        }
        Command::Motion(Motion::MatchBracket, extend) => {
            nav::match_bracket(app.active_doc_mut(), extend)
        }
        Command::SelectAll => nav::select_all(app.active_doc_mut()),
        Command::ScrollLineUp => nav_scroll::scroll_line_up(app.active_doc_mut()),
        Command::ScrollLineDown => nav_scroll::scroll_line_down(app.active_doc_mut()),
        Command::ScrollHalfPageUp => nav_scroll::scroll_half_page_up(app.active_doc_mut()),
        Command::ScrollHalfPageDown => nav_scroll::scroll_half_page_down(app.active_doc_mut()),
        Command::CentreCursor => nav_scroll::centre_cursor(app.active_doc_mut()),
        Command::CursorToTop => nav_scroll::cursor_to_top(app.active_doc_mut()),
        Command::CursorToBottom => nav_scroll::cursor_to_bottom(app.active_doc_mut()),
        Command::DeleteLeft => edit::delete_left(app, app.active),
        Command::DeleteRight => edit::delete_right(app, app.active),
        Command::DeleteWordLeft => edit::delete_word_left(app, app.active),
        Command::DeleteWordRight => edit::delete_word_right(app, app.active),
        Command::DeleteLine => edit_lines::delete_line(app, app.active),
        Command::Indent => edit_lines::indent(app, app.active),
        Command::Outdent => edit_lines::outdent(app, app.active),
        Command::MoveLineUp => edit_lines_move::move_line_up(app, app.active),
        Command::MoveLineDown => edit_lines_move::move_line_down(app, app.active),
        Command::CloneLineUp => edit_lines_move::clone_line_up(app, app.active),
        Command::CloneLineDown => edit_lines_move::clone_line_down(app, app.active),
        Command::AddCursorAbove => multi::add_cursor_above(app.active_doc_mut()),
        Command::AddCursorBelow => multi::add_cursor_below(app.active_doc_mut()),
        Command::Undo => edit::undo(app, app.active),
        Command::Redo => edit::redo(app, app.active),
        Command::Copy => clipboard::copy(app, app.active, effects),
        Command::Cut => clipboard::cut(app, app.active, effects),
        Command::Paste => clipboard::paste(effects, PasteTarget::Document(app.active)),
        Command::Save => {
            let _ = save::trigger_save(
                app,
                app.active,
                save::SaveMode::Normal,
                save::SaveOrigin::Interactive,
                effects,
            );
        }
        Command::FollowLink => navigate::follow(app, effects),
        Command::Reload => {
            match crate::registry::availability(app, crate::registry::CommandId::Editor(command)) {
                crate::registry::Availability::Available => {
                    crate::graphics::reload_image(app, app.active, effects);
                    crate::graphics::reload_embeds(app, app.active, effects);
                }
                crate::registry::Availability::Unavailable(reason) => {
                    crate::messages::info(app, reason);
                }
            }
        }
        Command::QuitConfirm => {
            if let Some(quit_key) = quit_key {
                pane::handle_quit_key(app, quit_key, effects);
            }
        }
    }
    keymap::KeyOutcome::Consumed
}

fn at_buffer_top(app: &App) -> bool {
    let doc = app.active_doc();
    let offset = doc.cursors.primary().position;
    doc.buffer.offset_to_line_col(offset).line == 0
}
