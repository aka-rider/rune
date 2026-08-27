//! Stage 2 of the four-stage key pipeline (`app::handle_key`): the
//! `GlobalCommand` handler match, and its small arm-support helpers — split
//! out of `pane.rs` to keep it under the 500-line budget. Per-command arm
//! bodies too large to inline live in `pane_global.rs`; the `BarPolicy`
//! table lives in `pane_bar_policy.rs`; the registry refusal check lives in
//! `pane_refusal.rs`.

use crate::app::App;
use crate::keymap::GlobalCommand;
use crate::messages;
use crate::pane_bar_policy::{self, BarPolicy};
use crate::pane_global;
use crate::pane_quit::handle_quit_key;
use crate::pane_refusal::registry_refusal;
use crate::runtime::Effects;
use crate::save;

/// Stage 2 of the four-stage key pipeline (`app::handle_key`): every
/// `GlobalCommand` fires regardless of which pane
/// currently has focus — the quit chords and Save in particular must keep
/// working while the Explorer/Tabs stub panes own it.
pub(crate) fn handle_global_command(app: &mut App, cmd: GlobalCommand, effects: &mut Effects) {
    // Captured BEFORE either hoisted gate below runs: `ToggleLeft`'s own
    // show/hide decision must reflect what was ACTUALLY painted the moment
    // this chord was pressed, not a layout that already changed underneath
    // it. The fuzzy file finder paints the left column via a `layout::
    // resolve` override that never touches `App::splits` — so closing an
    // open finder (the close-gate below, which lands focus back on the
    // Editor) leaves the raw `Split` flag exactly as bare as it was before
    // the finder ever opened. Re-deriving `layout_mode()` AFTER that close
    // would read that bare flag and conclude the column was never shown,
    // making `ToggleLeft` re-show it and steal focus right back — this
    // capture is what keeps that from happening.
    let left_painted_before = matches!(
        app.layout_mode(),
        crate::focus::LayoutMode::Split { .. } | crate::focus::LayoutMode::ExplorerOnly
    );

    // ONE hoisted gate, deliberately before the match:
    // a global chord pressed while the title is focused commits the typed
    // name FIRST, so ^S can never save under the old name and the edit is
    // never silently discarded. A no-op when the title isn't focused. NEVER
    // an early return: a refused commit leaves focus on the title with the
    // reason already in the footer, but every arm below must stay reachable
    // regardless — quit, save and close would otherwise be unreachable for a
    // user holding an unusable name. Blur is idempotent by design, which
    // is what keeps a repeated blur (each arm's own `set_focus` re-entering
    // `on_blur`) harmless.
    app.blur_title(effects);

    match pane_bar_policy::bar_policy(cmd) {
        BarPolicy::CloseBars => app.close_all_overlays(effects),
        BarPolicy::ToggleSearch => close_filesearch(app, effects),
        BarPolicy::LeaveOpen => {}
    }

    match cmd {
        // The single left-column toggle (Enter/Escape rework): painted this
        // frame ⇒ hide it and hand focus to the Editor; not painted ⇒ show
        // it, focus the Explorer, and land the cursor on the active
        // document's own file. Reads `left_painted_before` (`layout_mode()`
        // captured above, before this press's own effects), never the raw
        // `Split` flag, so a frame too narrow to actually paint the column
        // (flag still `shown`, nothing on screen — the exact shadow state
        // `focus::LayoutMode` exists to close) is treated as hidden, and a
        // press there shows rather than uselessly re-hiding an already
        // invisible column.
        GlobalCommand::ToggleLeft => pane_global::toggle_left(app, left_painted_before, effects),
        // Entering the title needs no `Effects` (it never leaves one) —
        // reseeded from the document actually showing, every time, so it
        // can never present a stale name from a previous or abandoned edit.
        GlobalCommand::FocusTitle => {
            run_if_available(app, cmd, effects, |app, _| app.focus_title())
        }
        // No dir-load side effect needed here — unlike Explorer, Tabs has
        // nothing to lazily fetch off-thread.
        GlobalCommand::FocusTabs => pane_global::focus_tabs(app, effects),
        GlobalCommand::Save => run_if_available(app, cmd, effects, |app, effects| {
            let _ = save::trigger_save(
                app,
                app.active,
                save::SaveMode::Normal,
                save::SaveOrigin::Interactive,
                effects,
            );
        }),
        GlobalCommand::Help => pane_global::help(app, effects),
        GlobalCommand::QuitChord(key) => handle_quit_key(app, key, effects),
        // Routes through the one close chokepoint regardless of which pane
        // held focus when `^w` was pressed, so a dirty document still arms
        // its Guard exactly like the Tabs-pane-local close it replaces.
        GlobalCommand::CloseFile => run_if_available(app, cmd, effects, |app, effects| {
            crate::workspace::request_close(app, app.active, effects);
        }),
        GlobalCommand::NewDocument => pane_global::new_document(app, effects),
        GlobalCommand::TabSwitch(idx) => pane_global::tab_switch(app, idx, effects),
        // No focus change or manual view invalidation needed — the
        // toggle's geometry change is absorbed by the next `view()` call.
        GlobalCommand::ToggleReadOnly => {
            run_if_available(app, cmd, effects, |app, _| {
                crate::commands::reading::toggle(app)
            });
        }
        // Starts a merge attempt, or exits an already-active
        // one in place — see `merge::toggle`'s own docs.
        GlobalCommand::Merge => run_if_available(app, cmd, effects, crate::merge::toggle),
        // The message log pane's own open/focus/collapse state
        // machine lives on `messages` itself, alongside every other reader/
        // writer of `App.messages`.
        GlobalCommand::ToggleMessages => messages::toggle(app, effects),
        GlobalCommand::Trash => pane_global::trash(app, effects),
        GlobalCommand::TogglePin => run_if_available(app, cmd, effects, |app, _| {
            crate::opentabs::limit::toggle_pin(app, app.active);
        }),
        GlobalCommand::ToggleSearch => pane_global::toggle_search(app, effects),
        // Reuses the cursor-jump `advance` Enter/Shift+Enter already drive:
        // open, this behaves like Enter/Shift+Enter; closed, `advance_closed`
        // recomputes matches from `last_search_query` and jumps without
        // painting highlights. Nothing to navigate with is never swallowed.
        GlobalCommand::SearchNext => search_step(app, true),
        GlobalCommand::SearchPrev => search_step(app, false),
        GlobalCommand::ToggleFileSearch => pane_global::toggle_file_search(app, effects),
        GlobalCommand::TogglePalette => pane_global::toggle_palette(app, effects),
        GlobalCommand::NavBack => crate::navhistory::back(app, effects),
        GlobalCommand::NavForward => crate::navhistory::forward(app, effects),
    }
}

/// The chokepoint every registry-gated arm above shares: consults `cmd`'s
/// own registry row first, posting its `Unavailable` reason through the
/// same poster the palette's greyed row would show, and running `action`
/// only once the row reads `Available`.
fn run_if_available(
    app: &mut App,
    cmd: GlobalCommand,
    effects: &mut Effects,
    action: impl FnOnce(&mut App, &mut Effects),
) {
    if let Some(reason) = registry_refusal(app, crate::registry::CommandId::Global(cmd)) {
        messages::error(app, reason);
    } else {
        action(app, effects);
    }
}

/// The finder-only half of [`App::close_all_overlays`] — `ToggleSearch`'s own arm
/// needs this without the search-bar half, since pre-closing the bar there
/// would make its own open/close branch always see it closed and reopen
/// instead of ever closing.
fn close_filesearch(app: &mut App, effects: &mut Effects) {
    if app.filesearch().is_some() {
        crate::filesearch::cancel(app, effects);
    }
}

/// The shared body of the `SearchNext`/`SearchPrev` arms above.
fn search_step(app: &mut App, forward: bool) {
    if app.search().is_some() {
        crate::search::keys::advance(app, forward);
    } else if !crate::search::keys::advance_closed(app, forward) {
        messages::info(app, "no previous search");
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::pane::Pane;
    use crate::runtime::{CmdKind, Msg};
    use rune_core::buffer::Buffer;
    use rune_vfs::Mem;
    use std::sync::Arc;

    fn app() -> App {
        App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None)
    }

    fn fx() -> Effects {
        Effects::default()
    }

    /// The show branch: a hidden column becomes shown and the Explorer
    /// takes focus.
    #[test]
    fn toggle_left_shows_the_column_and_focuses_the_explorer() {
        let mut app = app();
        let mut effects = Effects::default();
        handle_global_command(&mut app, GlobalCommand::ToggleLeft, &mut effects);
        assert!(app.splits.left.is_shown());
        assert_eq!(app.focus(), Pane::Explorer);
    }

    /// The hide branch: a visible, focused column collapses and focus
    /// returns to the Editor.
    #[test]
    fn toggle_left_hides_the_column_and_returns_focus_to_the_editor() {
        let mut app = app();
        let mut effects = Effects::default();
        handle_global_command(&mut app, GlobalCommand::ToggleLeft, &mut effects); // show
        handle_global_command(&mut app, GlobalCommand::ToggleLeft, &mut effects); // hide
        assert!(!app.splits.left.is_shown());
        assert_eq!(app.focus(), Pane::Editor);
    }

    /// The hide branch reaches focus through `focus::reconcile` itself now,
    /// not a hand-rolled equivalent — from a Tabs-focused column, the
    /// command path and a direct `reconcile` call after the same `hide()`
    /// must land on identical focus, proving the two really share the one
    /// chokepoint rather than two routes that happen to agree today.
    #[test]
    fn toggle_left_hide_branch_matches_a_direct_reconcile_call() {
        let mut via_command = app();
        let mut effects = Effects::default();
        via_command.splits.left.show();
        via_command.set_focus_pane(Pane::Tabs, &mut effects);
        handle_global_command(&mut via_command, GlobalCommand::ToggleLeft, &mut effects);

        let mut via_reconcile = app();
        let mut effects = Effects::default();
        via_reconcile.splits.left.show();
        via_reconcile.set_focus_pane(Pane::Tabs, &mut effects);
        via_reconcile.splits.left.hide();
        crate::focus::reconcile(&mut via_reconcile, &mut effects);

        assert_eq!(
            via_command.splits.left.is_shown(),
            via_reconcile.splits.left.is_shown()
        );
        assert_eq!(via_command.focus(), via_reconcile.focus());
        assert_eq!(via_command.focus(), Pane::Editor);
    }

    /// Pressing the single toggle twice is identity for both visibility and
    /// focus — never a dead key, and never a state a third press would need
    /// to "catch up" from.
    #[test]
    fn toggle_left_twice_is_identity() {
        let mut app = app();
        let before_shown = app.splits.left.is_shown();
        let before_focus = app.focus();
        let mut effects = Effects::default();
        handle_global_command(&mut app, GlobalCommand::ToggleLeft, &mut effects);
        handle_global_command(&mut app, GlobalCommand::ToggleLeft, &mut effects);
        assert_eq!(app.splits.left.is_shown(), before_shown);
        assert_eq!(app.focus(), before_focus);
    }

    /// The show branch shows the column, but a frame too small to paint
    /// anything in it at all — even full-width (`LayoutMode::ExplorerOnly`)
    /// — must never leave focus stranded on an invisible pane;
    /// `set_focus_pane` falls back to the Editor instead.
    #[test]
    fn toggle_left_on_a_too_small_frame_falls_back_to_the_editor() {
        let mut app = app();
        app.frame = Some(crate::app::FrameSize::new(5, 5));
        let mut effects = Effects::default();
        handle_global_command(&mut app, GlobalCommand::ToggleLeft, &mut effects);
        assert_eq!(app.focus(), Pane::Editor);
    }

    /// `NewDocument` mints a new untitled draft, activates it, and focuses
    /// the title field. No frame-size setup is needed: `Pane::Title` is
    /// focusable under every layout mode.
    #[test]
    fn new_document_mints_activates_and_focuses_the_title() {
        let mut app = app();
        let n = app.documents.len();
        let before = app.active;
        let mut effects = Effects::default();

        handle_global_command(&mut app, GlobalCommand::NewDocument, &mut effects);

        assert_eq!(app.documents.len(), n + 1);
        assert_ne!(app.active, before);
        assert_eq!(app.active_doc().display_name.as_deref(), Some("Untitled 1"));
        assert_eq!(app.focus(), Pane::Title);
    }

    /// Every focus-moving global closes an open search bar first — the bar
    /// is never a `Pane`, so leaving it open while one of these moves
    /// `App::focus` underneath it would strand keys on a bar nothing is
    /// painting a caret for anymore.
    #[test]
    fn every_focus_moving_global_closes_an_open_search_bar() {
        for cmd in [
            GlobalCommand::ToggleLeft,
            GlobalCommand::FocusTitle,
            GlobalCommand::FocusTabs,
            GlobalCommand::ToggleMessages,
            GlobalCommand::Merge,
        ] {
            let mut app = app();
            crate::search::open(&mut app, &mut fx());
            assert!(app.search().is_some(), "test setup: bar is open");

            let mut effects = Effects::default();
            handle_global_command(&mut app, cmd, &mut effects);

            assert!(app.search().is_none(), "{cmd:?} must close the search bar");
        }
    }

    /// The plan's own acceptance test: `⌘⇧F` opens the finder on a FRESH
    /// app whose left column was NEVER shown — pins the load-bearing
    /// ordering (`app.filesearch` assigned before `set_focus_pane`)
    /// together with the layout override that makes the (default-hidden)
    /// column paint anyway, driven through the real `App::update` seam.
    #[test]
    fn sup_shift_f_chord_opens_filesearch_on_a_never_shown_left_column() {
        let mut app = app();
        app.frame = Some(crate::app::FrameSize::new(120, 34));
        assert!(!app.splits.left.is_shown(), "test setup: column hidden");
        let mut effects = Effects::default();

        crate::app::update(
            &mut app,
            Msg::Key(crate::keymap::KeyInput {
                code: crate::keymap::KeyCode::Char('F'),
                mods: crate::keymap::Mods {
                    shift: false,
                    alt: false,
                    ctrl: false,
                    sup: true,
                },
            }),
            &mut effects,
        );

        assert!(app.filesearch().is_some());
        assert_eq!(app.focus(), Pane::Explorer);
    }

    /// A second `⌘⇧F` closes the finder and restores whatever document was
    /// active before it opened.
    #[test]
    fn sup_shift_f_chord_again_closes_and_restores_return_to() {
        let mut app = app();
        app.frame = Some(crate::app::FrameSize::new(120, 34));
        let second = app.open_document(rune_core::buffer::Buffer::new("second"));
        crate::workspace::switch_to(&mut app, second);
        let mut effects = Effects::default();
        let chord = crate::keymap::KeyInput {
            code: crate::keymap::KeyCode::Char('F'),
            mods: crate::keymap::Mods {
                shift: false,
                alt: false,
                ctrl: false,
                sup: true,
            },
        };

        crate::app::update(&mut app, Msg::Key(chord), &mut effects);
        assert!(app.filesearch().is_some(), "test setup: finder open");

        crate::app::update(&mut app, Msg::Key(chord), &mut effects);

        assert!(app.filesearch().is_none());
        assert_eq!(app.active, second);
        assert_eq!(app.focus(), Pane::Editor);
    }

    /// Table-driven close-gate invariant: for every command
    /// reachable from `GLOBAL_BINDINGS`, running it with the finder open
    /// must never leave it open with focus somewhere other than the
    /// Explorer — the exact "stage 3 swallows keys for a mode the user
    /// thinks they left" defect the close-gate exists to close. Table-
    /// driven over the real binding table (not a hand-picked subset) so a
    /// future global command is auto-covered the day its row is added. The
    /// document is left pathless (the default fresh-`App` draft) and made
    /// dirty, on purpose: `Save`'s own pathless-draft rung — focusing the
    /// title field directly, bypassing `set_focus_pane` — only fires on a
    /// dirty document, and is exactly the rung that strands the finder if
    /// `Save` is ever missing from the close-gate. Binding a path, or
    /// leaving the document clean, would make the table blind to it.
    #[test]
    fn table_driven_close_gate_never_strands_filesearch_off_explorer() {
        use crate::keymap::GLOBAL_BINDINGS;

        for binding in GLOBAL_BINDINGS {
            let mut app = app();
            app.frame = Some(crate::app::FrameSize::new(120, 34));
            let active = app.active;
            crate::commands::edit::insert_char(&mut app, active, '!');
            let mut effects = Effects::default();
            crate::filesearch::open(&mut app, &mut effects);
            assert!(
                app.filesearch().is_some(),
                "test setup for {:?}",
                binding.cmd
            );

            handle_global_command(&mut app, binding.cmd, &mut effects);

            assert!(
                app.filesearch().is_none() || app.focus() == Pane::Explorer,
                "{:?} left the finder open with focus on {:?}",
                binding.cmd,
                app.focus()
            );
        }
    }

    /// `Trash` while the finder is open must never trash the merely-
    /// arrowed-past active document: it behaves like Esc instead (closes,
    /// restores `return_to`) and explains why, rather than silently
    /// discarding the chord.
    #[test]
    fn trash_while_filesearch_is_open_closes_it_instead_of_trashing() {
        let mut app = app();
        let second = app.open_document(rune_core::buffer::Buffer::new("second"));
        crate::workspace::switch_to(&mut app, second);
        let mut effects = Effects::default();
        crate::filesearch::open(&mut app, &mut effects);

        handle_global_command(&mut app, GlobalCommand::Trash, &mut effects);

        assert!(app.filesearch().is_none());
        assert_eq!(app.active, second);
        assert_eq!(app.focus(), Pane::Editor);
        assert!(
            effects.cmds.iter().all(|c| c.kind() != CmdKind::Trash),
            "no trash Cmd must be spawned while the finder was open"
        );
        assert!(
            messages::newest_text(&app).is_some(),
            "a message was posted"
        );
    }

    /// Regression: `^B` pressed while the finder is open, on a column
    /// whose OWN `Split` was never actually shown (the finder paints it via
    /// `layout::resolve`'s override alone), must land on the Editor and
    /// leave the column collapsed — not close the finder via the hoisted
    /// close-gate and then immediately re-show the column and steal focus
    /// back to the Explorer, which is what re-deriving `layout_mode()`
    /// AFTER the close used to do (a fuzz-caught `UNDO-TOTAL` stall: focus
    /// stuck off the Editor at session end, so no `⌘Z` could ever reach the
    /// journal).
    #[test]
    fn toggle_left_while_filesearch_is_open_on_a_never_shown_column_lands_on_the_editor() {
        let mut app = app();
        app.frame = Some(crate::app::FrameSize::new(120, 34));
        assert!(
            !app.splits.left.is_shown(),
            "test setup: column never shown"
        );
        let mut effects = Effects::default();
        crate::filesearch::open(&mut app, &mut effects);
        assert!(app.filesearch().is_some(), "test setup: finder open");
        assert_eq!(
            app.focus(),
            Pane::Explorer,
            "test setup: finder's own override"
        );

        handle_global_command(&mut app, GlobalCommand::ToggleLeft, &mut effects);

        assert!(app.filesearch().is_none());
        assert_eq!(app.focus(), Pane::Editor);
        assert!(
            !app.splits.left.is_shown(),
            "the column must stay collapsed — its own Split was never really shown"
        );
    }

    /// `^F` while a merge is active on the active document refuses to open
    /// the bar, with feedback, and touches no state — the merge resolver
    /// claims every editor key for itself, so a bar opening on top of it
    /// would silently steal keys the resolver never gets a chance to see.
    #[test]
    fn toggle_search_refuses_to_open_during_an_active_merge() {
        let mut app = app();
        app.merge = crate::merge::MergeState::Active {
            doc: app.active,
            session: crate::merge::MergeSession {
                conflicts: Vec::new(),
                cur: 0,
                saved_display_name: None,
                theirs_obs: rune_db::ObsId::new(1).expect("nonzero"),
                install_pos: 0,
            },
        };
        let mut effects = Effects::default();

        handle_global_command(&mut app, GlobalCommand::ToggleSearch, &mut effects);

        assert!(app.search().is_none(), "the bar must not open mid-merge");
        assert_eq!(
            messages::newest_text(&app),
            Some("finish the merge first (^M)")
        );
    }
}
