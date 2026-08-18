//! Keystroke handling for the fuzzy file finder overlay — reached from
//! `dispatch::handle_key`'s stage 3 whenever `focus::target` resolves to
//! `FocusTarget::FileSearch`, ahead of the ordinary chrome-level `Pane`
//! match, since the finder is never itself a `Pane`.
//!
//! Every path returns [`KeyOutcome::Consumed`] — the same discipline the
//! in-file search bar's own key handling uses (`search::keys`), so a
//! keystroke aimed at the finder never falls through and mutates the
//! document buffer underneath it.

use std::path::PathBuf;

use unicode_segmentation::UnicodeSegmentation;

use crate::app::App;
use crate::binding::{Binding, KeyPattern, resolve_in};
use crate::keymap::{KeyCode, KeyInput, KeyOutcome, Mods};
use crate::pane::Pane;
use crate::runtime::Effects;

use super::{after_cursor_move, cancel, close, reset_and_recompute};

const SHIFT: Mods = Mods {
    shift: true,
    alt: false,
    ctrl: false,
    sup: false,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileSearchCommand {
    Type,
    Erase,
    Up,
    Down,
    PageUp,
    PageDown,
    Top,
    Bottom,
    Open,
    Cancel,
}

/// The finder's own key table. Two printable rows (`Mods::NONE`, `SHIFT`)
/// let the very first keystroke both start filtering and supply its first
/// character, mirroring `EXPLORER_SEARCH_BINDINGS`'s own `Type` row.
pub const FILESEARCH_BINDINGS: &[Binding<FileSearchCommand>] = &[
    Binding {
        key: KeyPattern::printable(Mods::NONE),
        cmd: FileSearchCommand::Type,
        help: "type to filter",
        secondary: false,
    },
    Binding {
        key: KeyPattern::printable(SHIFT),
        cmd: FileSearchCommand::Type,
        help: "type to filter",
        secondary: true,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Backspace, Mods::NONE),
        cmd: FileSearchCommand::Erase,
        help: "erase",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Up, Mods::NONE),
        cmd: FileSearchCommand::Up,
        help: "up",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Down, Mods::NONE),
        cmd: FileSearchCommand::Down,
        help: "down",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::PageUp, Mods::NONE),
        cmd: FileSearchCommand::PageUp,
        help: "page up",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::PageDown, Mods::NONE),
        cmd: FileSearchCommand::PageDown,
        help: "page down",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Home, Mods::NONE),
        cmd: FileSearchCommand::Top,
        help: "top",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::End, Mods::NONE),
        cmd: FileSearchCommand::Bottom,
        help: "bottom",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Enter, Mods::NONE),
        cmd: FileSearchCommand::Open,
        help: "open",
        secondary: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Escape, Mods::NONE),
        cmd: FileSearchCommand::Cancel,
        help: "cancel",
        secondary: false,
    },
];

pub(crate) fn handle_key(app: &mut App, key: KeyInput, effects: &mut Effects) -> KeyOutcome {
    if let Some(cmd) = resolve_in(FILESEARCH_BINDINGS, key) {
        apply(app, cmd, key, effects);
    }
    KeyOutcome::Consumed
}

fn apply(app: &mut App, cmd: FileSearchCommand, key: KeyInput, effects: &mut Effects) {
    match cmd {
        FileSearchCommand::Type => {
            if let KeyCode::Char(c) = key.code {
                if let Some(state) = app.filesearch_mut() {
                    state.query.push(c);
                }
                reset_and_recompute(app, effects);
            }
        }
        FileSearchCommand::Erase => {
            erase(app);
            reset_and_recompute(app, effects);
        }
        FileSearchCommand::Up => nav_move(app, -1, effects),
        FileSearchCommand::Down => nav_move(app, 1, effects),
        FileSearchCommand::PageUp => nav_move(app, -page_amount(app), effects),
        FileSearchCommand::PageDown => nav_move(app, page_amount(app), effects),
        FileSearchCommand::Top => {
            if let Some(state) = app.filesearch_mut() {
                state.nav.first();
            }
            after_cursor_move(app, effects);
        }
        FileSearchCommand::Bottom => {
            if let Some(state) = app.filesearch_mut() {
                let len = state.results.len();
                state.nav.last(len);
            }
            after_cursor_move(app, effects);
        }
        FileSearchCommand::Open => open_selected(app, effects),
        FileSearchCommand::Cancel => cancel(app, effects),
    }
}

/// `Enter`: opens the selected candidate. If the nav cursor's own live
/// preview already loaded this exact file (`app.explorer.preview`, the
/// SAME slot the Explorer's own preview uses — the finder rides that
/// machinery rather than inventing a second one), promotes it in place
/// instead of re-reading it, mirroring `explorer_keys::open_selected`'s own
/// promote branch. Otherwise opens through the same tab-cap-respecting
/// chokepoint the Explorer's own `Open` uses (`workspace::
/// open_path_checked`) — [`close`] runs only AFTER that succeeds, never
/// before: closing first and reading second would strand the user with the
/// finder gone, focus wherever `close` happened to leave it, and `return_to`
/// lost the moment a read fails. A read failure is already reported by
/// `open_path_checked` itself and leaves the finder open so the user can
/// pick another candidate; nothing selected (an empty result list, the
/// cursor past the end) reports through the message log the same way.
fn open_selected(app: &mut App, effects: &mut Effects) {
    let Some(path) = selected_path(app) else {
        crate::messages::info(app, "no file selected");
        return;
    };
    let departed = app.filesearch().map(|state| state.return_to);

    if let Some(id) = app.explorer.preview
        && app.doc(id).and_then(|d| d.file_path.as_deref()) == Some(path.as_path())
    {
        close(app);
        crate::explorer_preview::promote(app, id);
        app.set_focus_pane(Pane::Editor, effects);
        crate::navhistory::record_departure_if_moved(app, departed);
        return;
    }

    if crate::workspace::open_path_checked(app, &path, effects).is_some() {
        close(app);
        app.set_focus_pane(Pane::Editor, effects);
        crate::navhistory::record_departure_if_moved(app, departed);
    }
}

fn selected_path(app: &App) -> Option<PathBuf> {
    super::selected_candidate(app).map(|c| c.path.clone())
}

/// Erases one GRAPHEME CLUSTER, not one `char` — the same reasoning
/// `search::keys::erase`/`explorer_search::handle_search`'s own `Erase` arm
/// apply, so a combining mark popped alone never desyncs what's on screen
/// from what the query actually holds.
fn erase(app: &mut App) {
    let Some(state) = app.filesearch_mut() else {
        return;
    };
    if let Some((byte_idx, _)) = state.query.grapheme_indices(true).next_back() {
        state.query.truncate(byte_idx);
    }
}

pub(super) fn nav_move(app: &mut App, delta: isize, effects: &mut Effects) {
    let height = page_amount(app).max(1) as usize;
    let margin = (height / 4).min(4);
    let Some(state) = app.filesearch_mut() else {
        return;
    };
    let len = state.results.len();
    state.nav.move_by(delta, len);
    state.nav.follow(len, height, margin, 0);
    after_cursor_move(app, effects);
}

/// The finder's visible result-row count, read straight from
/// `layout::geometry`'s `explorer_inner` (the rect the finder replaces the
/// Explorer's own content in) minus the one row the query bar occupies —
/// same derivation shape as `explorer::visible_rows`. `pub(super)`: also
/// `recompute_core`'s own follow-the-cursor scroll math (`mod.rs`) needs it.
pub(super) fn page_amount(app: &App) -> isize {
    let area = ratatui::layout::Rect::new(0, 0, app.frame_width, app.frame_height);
    (crate::layout::geometry(area, app).explorer_inner.height as isize)
        .saturating_sub(1)
        .max(1)
}

/// Appends pasted text to the query — the finder's counterpart of
/// `search::keys::paste`. Dropped outright once the finder has since
/// closed: a reply landing after Escape has nowhere left to append to.
/// Sanitized the same way ordinary typing is, first line only — the query
/// is rendered as a single row.
pub(crate) fn paste(app: &mut App, text: &str, effects: &mut Effects) {
    if app.filesearch().is_none() {
        return;
    }
    let sanitized: String = text
        .lines()
        .next()
        .unwrap_or("")
        .chars()
        .filter(|c| !c.is_control())
        .collect();
    if sanitized.is_empty() {
        return;
    }
    if let Some(state) = app.filesearch_mut() {
        state.query.push_str(&sanitized);
    }
    reset_and_recompute(app, effects);
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::filesearch::Candidate;
    use crate::focus::{self, FocusTarget};
    use rune_core::buffer::Buffer;
    use rune_vfs::{Mem, Vfs};
    use std::sync::Arc;

    fn app() -> App {
        let mut app = App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None);
        app.frame_width = 120;
        app.frame_height = 34;
        app
    }

    fn char_key(c: char) -> KeyInput {
        KeyInput {
            code: KeyCode::Char(c),
            mods: Mods::NONE,
        }
    }

    fn enter_key() -> KeyInput {
        KeyInput {
            code: KeyCode::Enter,
            mods: Mods::NONE,
        }
    }

    #[test]
    fn typing_appends_to_the_query() {
        let mut app = app();
        let mut effects = Effects::default();
        crate::filesearch::open(&mut app, &mut effects);

        assert_eq!(
            handle_key(&mut app, char_key('a'), &mut effects),
            KeyOutcome::Consumed
        );
        assert_eq!(
            handle_key(&mut app, char_key('b'), &mut effects),
            KeyOutcome::Consumed
        );

        assert_eq!(app.filesearch().map(|s| s.query.as_str()), Some("ab"));
    }

    #[test]
    fn escape_cancels_and_restores_focus() {
        let mut app = app();
        let mut effects = Effects::default();
        crate::filesearch::open(&mut app, &mut effects);

        let escape = KeyInput {
            code: KeyCode::Escape,
            mods: Mods::NONE,
        };
        assert_eq!(
            handle_key(&mut app, escape, &mut effects),
            KeyOutcome::Consumed
        );

        assert!(app.filesearch().is_none());
        assert_eq!(focus::target(&app), FocusTarget::Editor);
    }

    /// The plan's own acceptance test, driven through the real `App::
    /// update` seam: Esc closes the finder, restores the document that was
    /// active before it opened, and focuses the Editor.
    #[test]
    fn escape_through_app_update_restores_the_previously_active_document() {
        let mut app = app();
        let second = app.open_document(Buffer::new("second"));
        crate::workspace::switch_to(&mut app, second);
        let mut effects = Effects::default();
        crate::filesearch::open(&mut app, &mut effects);

        crate::app::update(
            &mut app,
            crate::runtime::Msg::Key(KeyInput {
                code: KeyCode::Escape,
                mods: Mods::NONE,
            }),
            &mut effects,
        );

        assert!(app.filesearch().is_none());
        assert_eq!(app.active, second);
        assert_eq!(app.focus(), crate::pane::Pane::Editor);
    }

    #[test]
    fn enter_opens_the_selected_candidate_and_returns_to_the_editor() {
        let mem = Arc::new(Mem::new());
        mem.save_atomic(std::path::Path::new("/root/a.md"), b"hello world")
            .expect("seed file");
        let vfs: Arc<dyn Vfs + Send + Sync> = mem;
        let mut app = App::new(Buffer::new("hello"), None, vfs, None);
        app.frame_width = 120;
        app.frame_height = 34;
        let mut effects = Effects::default();
        crate::filesearch::open(&mut app, &mut effects);
        let generation = app.filesearch().expect("open").generation;
        crate::filesearch::handle_recents_loaded(
            &mut app,
            generation,
            Ok(vec![Candidate {
                path: PathBuf::from("/root/a.md"),
                display: "a.md".to_string(),
                in_tree: true,
                mru_rank: None,
            }]),
            &mut effects,
        );

        assert_eq!(
            handle_key(&mut app, enter_key(), &mut effects),
            KeyOutcome::Consumed
        );

        assert!(app.filesearch().is_none());
        assert_eq!(app.focus(), crate::pane::Pane::Editor);
        assert_eq!(
            app.active_doc().file_path.as_deref(),
            Some(std::path::Path::new("/root/a.md"))
        );
    }

    #[test]
    fn enter_with_nothing_selected_reports_and_stays_open() {
        let mut app = app();
        let mut effects = Effects::default();
        crate::filesearch::open(&mut app, &mut effects);

        assert_eq!(
            handle_key(&mut app, enter_key(), &mut effects),
            KeyOutcome::Consumed
        );

        assert!(
            app.filesearch().is_some(),
            "nothing selected must never close the finder"
        );
        assert_eq!(crate::messages::newest_text(&app), Some("no file selected"));
    }

    /// Finding 11: a candidate whose path fails to read must not strand the
    /// user — the finder must stay open (its own `return_to` still intact)
    /// with the read failure reported, rather than closing before the read
    /// is even attempted and leaving focus wherever `close` happened to
    /// land it.
    #[test]
    fn enter_on_a_candidate_that_fails_to_read_leaves_the_finder_open_with_a_message() {
        let mut app = app();
        let mut effects = Effects::default();
        crate::filesearch::open(&mut app, &mut effects);
        let generation = app.filesearch().expect("open").generation;
        crate::filesearch::handle_recents_loaded(
            &mut app,
            generation,
            Ok(vec![Candidate {
                path: PathBuf::from("/root/missing.md"),
                display: "missing.md".to_string(),
                in_tree: true,
                mru_rank: None,
            }]),
            &mut effects,
        );

        assert_eq!(
            handle_key(&mut app, enter_key(), &mut effects),
            KeyOutcome::Consumed
        );

        assert!(
            app.filesearch().is_some(),
            "a failed open must leave the finder open rather than stranding the user"
        );
        assert_eq!(app.focus(), crate::pane::Pane::Explorer);
        assert!(
            crate::messages::newest_text(&app).is_some_and(|m| m.contains("could not open")),
            "the read failure must be reported"
        );
    }
}
