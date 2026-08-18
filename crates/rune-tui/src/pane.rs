//! `Pane` — the focus discriminant only, no trait objects; `Explorer`/
//! `Tabs`'s own state lands in plain named `App` fields. Extracted out of
//! `app.rs` to keep it under the 500-line budget — `handle_global_command`
//! lives here too since it's the sole reader/writer of `App::focus`/the
//! left column's `Split` state outside `app.rs` itself.

use std::time::Duration;

use crate::app::App;
use crate::document::DocumentId;
use crate::explorer;
use crate::guard::{self, GuardKind, GuardPrompt};
use crate::keymap::{GlobalCommand, QuitKey};
use crate::messages;
use crate::pane_global;
use crate::registry::{self, Availability, CommandId};
use crate::runtime::{Effects, Msg};
use crate::save;

fn registry_refusal(app: &App, cmd: GlobalCommand) -> Option<String> {
    match registry::availability(app, CommandId::Global(cmd)) {
        Availability::Available => None,
        Availability::Unavailable(reason) => Some(reason.into_owned()),
    }
}

/// The quit-confirm arm-to-quit window: the first press arms `App::timers`'
/// `TimerKey::QuitConfirm` deadline, carrying the confirm generation.
const CONFIRM_TIMEOUT: Duration = Duration::from_secs(2);

/// Which chrome region owns the next keystroke once the global table
/// (`keymap::GLOBAL_BINDINGS`) doesn't claim it — the pane-routing stage
/// of the key pipeline, after the global table and before pane-local keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    Explorer,
    Tabs,
    Editor,
    /// The editable title field (`title.rs`) — focused by `^r` or by
    /// pressing Up at the top of the editor. While it owns focus every
    /// keystroke goes to the file name and none of them reach the buffer.
    Title,
    /// The collapsible message-log pane above the footer — focused by
    /// `^E`/`⌘E` while the pane is open, or by clicking inside it. Only
    /// ever focusable while `messages::is_open` is true
    /// (`LayoutMode::focusable`'s own gate).
    Messages,
}

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

    match bar_policy(cmd) {
        BarPolicy::CloseBars => close_modal_bars(app, effects),
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
        // Entering the title needs no `Effects` — it can never itself
        // leave any. Reseeds from the document that is actually
        // showing, every time: the field must never present a stale name
        // from a previous document or a previously abandoned edit (no
        // shadow state).
        GlobalCommand::FocusTitle => app.focus_title(),
        // No dir-load side effect needed here — unlike Explorer, Tabs has
        // nothing to lazily fetch off-thread.
        GlobalCommand::FocusTabs => pane_global::focus_tabs(app, effects),
        GlobalCommand::Save => {
            let _ = save::trigger_save(app, app.active, save::SaveMode::Normal, effects);
        }
        GlobalCommand::Help => pane_global::help(app, effects),
        GlobalCommand::QuitChord(key) => handle_quit_key(app, key),
        // Routes through the one close chokepoint regardless of which pane
        // held focus when `^w` was pressed, so a dirty document still arms
        // its Guard exactly like the Tabs-pane-local close it replaces.
        GlobalCommand::CloseFile => crate::workspace::request_close(app, app.active, effects),
        GlobalCommand::NewDocument => pane_global::new_document(app, effects),
        GlobalCommand::TabSwitch(idx) => pane_global::tab_switch(app, idx, effects),
        // No focus change and no manual view invalidation needed — the
        // toggle's geometry change is absorbed by the next `view()` call
        // (`commands::reading`'s own docs).
        GlobalCommand::ToggleReadOnly => {
            if let Some(reason) = registry_refusal(app, cmd) {
                messages::warn(app, reason);
            } else {
                crate::commands::reading::toggle(app);
            }
        }
        // Starts a merge attempt, or exits an already-active
        // one in place — see `merge::toggle`'s own docs.
        GlobalCommand::Merge => {
            if let Some(reason) = registry_refusal(app, cmd) {
                messages::warn(app, reason);
            } else {
                crate::merge::toggle(app, effects);
            }
        }
        // The message log pane's own open/focus/collapse state
        // machine lives on `messages` itself, alongside every other reader/
        // writer of `App.messages`.
        GlobalCommand::ToggleMessages => messages::toggle(app, effects),
        GlobalCommand::Trash => pane_global::trash(app, effects),
        GlobalCommand::TogglePin => {
            if let Some(reason) = registry_refusal(app, cmd) {
                messages::warn(app, reason);
            } else {
                crate::opentabs::limit::toggle_pin(app, app.active);
            }
        }
        GlobalCommand::ToggleSearch => pane_global::toggle_search(app, effects),
        // Reuses the same cursor-jump `advance` Enter/Shift+Enter already
        // drive: with the bar open, this chord behaves exactly like
        // Enter/Shift+Enter would; with it closed, `advance_closed`
        // recomputes matches from `App::last_search_query` on demand and
        // jumps without painting highlights.
        // Nothing to navigate with is reported, never swallowed silently.
        GlobalCommand::SearchNext => search_step(app, true),
        GlobalCommand::SearchPrev => search_step(app, false),
        GlobalCommand::ToggleFileSearch => pane_global::toggle_file_search(app, effects),
        GlobalCommand::TogglePalette => pane_global::toggle_palette(app, effects),
        GlobalCommand::NavBack => crate::navhistory::back(app, effects),
        GlobalCommand::NavForward => crate::navhistory::forward(app, effects),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BarPolicy {
    CloseBars,
    ToggleSearch,
    LeaveOpen,
}

fn bar_policy(cmd: GlobalCommand) -> BarPolicy {
    match cmd {
        GlobalCommand::ToggleLeft
        | GlobalCommand::FocusTitle
        | GlobalCommand::FocusTabs
        | GlobalCommand::ToggleMessages
        | GlobalCommand::Merge
        | GlobalCommand::Help
        | GlobalCommand::NewDocument
        | GlobalCommand::TabSwitch(_)
        | GlobalCommand::CloseFile
        | GlobalCommand::NavBack
        | GlobalCommand::NavForward
        | GlobalCommand::Save => BarPolicy::CloseBars,
        GlobalCommand::ToggleSearch => BarPolicy::ToggleSearch,
        GlobalCommand::QuitChord(_)
        | GlobalCommand::ToggleReadOnly
        | GlobalCommand::Trash
        | GlobalCommand::SearchNext
        | GlobalCommand::SearchPrev
        | GlobalCommand::TogglePin
        | GlobalCommand::ToggleFileSearch
        | GlobalCommand::TogglePalette => BarPolicy::LeaveOpen,
    }
}

/// Closes both modal overlays — the in-file search bar and the fuzzy file
/// finder — for every global that moves focus, switches the active
/// document, or opens another surface. The finder closes via
/// `filesearch::cancel` rather than a bare `app.filesearch = None`, so its
/// own focus/return-to restore stays coherent instead of leaving `App::
/// focus` wherever the caller's own arm is about to move it. Private —
/// called only from this module's own `handle_global_command` above.
fn close_modal_bars(app: &mut App, effects: &mut Effects) {
    crate::search::close(app);
    close_filesearch(app, effects);
    close_palette(app);
}

fn close_palette(app: &mut App) {
    if app.palette().is_some() {
        crate::palette::close(app);
    }
}

/// The finder-only half of [`close_modal_bars`] — `ToggleSearch`'s own arm
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

/// Shows the left column, focuses the Explorer, and lands the cursor on the
/// active document's own file — the ONE chokepoint both `ToggleLeft`'s show
/// branch (above) and the editor's Escape cascade (`dispatch::
/// handle_editor_key`) reach through, so the two triggers can never drift
/// apart on how "reveal the active file" behaves. A document with no
/// `file_path` (a draft, or the virtual Help document) has nothing to
/// reveal, so it falls back to the Explorer's ordinary first-load fill
/// instead of calling `explorer_reveal::reveal`.
pub(crate) fn show_and_focus_explorer_on_active_file(app: &mut App, effects: &mut Effects) {
    app.splits.left.show();
    app.splits.explorer.show();
    app.set_focus_pane(Pane::Explorer, effects);
    match app.active_doc().file_path.clone() {
        Some(path) => crate::explorer_reveal::reveal(app, &path, effects),
        None => explorer::ensure_loaded(app, effects),
    }
}

/// The quit-confirm state machine: the SAME
/// chord pressed twice quits; pressing a quit chord while a DIFFERENT one is
/// pending re-arms with the new chord and a fresh generation, restarting the
/// 2s window. `pub(crate)` — moved out of `app.rs` (500-line
/// budget); `handle_global_command` above is its only caller now that quit
/// chords resolve at the global pipeline stage.
pub(crate) fn handle_quit_key(app: &mut App, key: QuitKey) {
    // Quit is an implicit Esc for an active OR
    // pending merge — exited/cancelled BEFORE the dirty-guard scan below,
    // so that scan (and the guard prompt it may raise) sees the reverted
    // title/plain dirty text, never a stale "editor <-> disk" name for a
    // merge quit is about to end anyway. `auto_exit` (review fix F3)
    // cancels a `Pending` attempt WITH feedback instead of silently
    // discarding it.
    if !matches!(app.merge, crate::merge::MergeState::Inactive) {
        crate::merge::auto_exit(app);
    }
    // Quit is a destructive transition on every dirty document at once, and
    // the 2-press confirm above is only a safe shortcut BECAUSE quit
    // preserves through the durable journal. That premise
    // fails for any dirty document with no live `db` binding (the default
    // untitled draft by construction, or an Explorer/CLI-opened document
    // whose hydration never landed) — for those, quitting would discard
    // work with no journal to recover it from. Raise a Guard instead of
    // arming or completing quit. It carries `DirtyQuit`, not `DirtyClose`:
    // the answer must finish the quit the user asked for (discard exits;
    // save exits once every started save acks), because a Guard whose
    // answers only ever CLOSED left a single-document session with no
    // reachable exit at all.
    if let Some(doc) = unpreserved_dirty_docs(app).into_iter().next() {
        let _ = guard::set_guard_or_warn(
            app,
            GuardPrompt {
                doc,
                kind: GuardKind::DirtyQuit,
            },
            "quit confirmation dropped \u{2014} a prompt is already showing",
        );
        return;
    }

    if let crate::app::QuitNegotiation::ConfirmArmed(pending_key, generation) = app.quit
        && pending_key == key
    {
        let _ = generation; // the SAME chord always quits regardless of generation
        app.should_quit = true;
        return;
    }

    let generation = app.next_quit_gen.mint();
    app.quit = crate::app::QuitNegotiation::ConfirmArmed(key, generation);
    app.timers.arm(
        crate::runtime::TimerKey::QuitConfirm,
        CONFIRM_TIMEOUT,
        Msg::ConfirmTimeout { generation },
    );
}

/// Every open document that is both dirty and has no live, trustworthy
/// recovery-store binding (`App::is_preserved`) — quit preserves through the
/// durable journal, so a dirty document without one is the exact case
/// `handle_quit_key`'s Guard gate exists for, and the exact set the
/// quit-save fan-out (`guard`'s `[S]ave` answer) must save every
/// member of, not just the first. Deterministic ordering (`documents` is a
/// `BTreeMap`) rather than "whichever `HashMap` bucket happens to iterate
/// first" — repeated presses always raise the Guard for the same document
/// until it's resolved. Dirty is re-derived via `is_dirty_now`, not read
/// from the cache — quit is a transition — so a stale
/// cache can never wave a genuinely-dirty document through the guard.
/// `handle_quit_key`'s own Guard-raise takes just the first (lowest-id) one;
/// the quit-save fan-out iterates the whole `Vec`.
pub(crate) fn unpreserved_dirty_docs(app: &mut App) -> Vec<DocumentId> {
    let candidates: Vec<DocumentId> = app.documents.keys().copied().collect();
    candidates
        .into_iter()
        .filter(|&id| {
            let preserved = app.doc(id).is_some_and(|d| app.is_preserved(d));
            !preserved && crate::materialize_ack::is_dirty_now(app, id)
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::db::{Db, DbBridge};
    use crate::document::Replica;
    use crate::runtime::CmdKind;
    use rune_core::buffer::Buffer;
    use rune_db::{ClockFn, Store};
    use rune_vfs::{Mem, Vfs};
    use std::sync::Arc;

    fn app() -> App {
        App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None)
    }

    /// A live, non-degraded app-level `Db` (mirrors `db_ack.rs::tests::
    /// in_memory_db`) — `App::is_preserved` requires one to exist (not just
    /// a document's own `DocDb`) before it will call a document preserved,
    /// since `degraded` lives on the app-level handle.
    fn live_db() -> Db {
        let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Mem::new());
        let clock: ClockFn = Arc::new(std::time::SystemTime::now);
        let store = Store::open_in_memory(clock, vfs, Box::new(|_evt| {})).expect("open store");
        let bridge = DbBridge::bootstrap();
        Db::new(store, bridge, false)
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
        app.frame_width = 5;
        app.frame_height = 5;
        let mut effects = Effects::default();
        handle_global_command(&mut app, GlobalCommand::ToggleLeft, &mut effects);
        assert_eq!(app.focus(), Pane::Editor);
    }

    /// A dirty document with no live
    /// `db` binding (the default for an untitled draft) must never be
    /// silently discarded by the quit chord — `^C^C` (or `^D^D`) raises a
    /// `DirtyQuit` Guard rather than quitting or merely
    /// closing.
    #[test]
    fn double_quit_chord_on_an_unpreserved_dirty_doc_raises_a_guard_instead_of_quitting() {
        let mut app = app();
        // Dirty is a content comparison — poking the render-
        // only cache directly would just be overwritten by `is_dirty_now`'s
        // re-derive, so diverge `saved_content` from the live buffer
        // instead, exactly like a real edit would.
        app.doc_mut(app.active)
            .expect("active doc exists")
            .saved_content = Arc::from("");
        assert!(
            !app.active_doc().is_store_bound(),
            "test setup: no db binding"
        );

        handle_quit_key(&mut app, QuitKey::CtrlC);
        handle_quit_key(&mut app, QuitKey::CtrlC);

        assert!(
            !app.should_quit,
            "quit must not complete while unpreserved dirty work exists"
        );
        assert!(
            matches!(
                app.guard,
                Some(GuardPrompt {
                    kind: GuardKind::DirtyQuit,
                    ..
                })
            ),
            "expected a DirtyQuit Guard prompt to be raised"
        );
    }

    /// The converse: a dirty document that IS preserved (has a live `db`
    /// binding) doesn't trip the new gate — the ordinary two-press
    /// quit-confirm still works exactly as before.
    #[test]
    fn double_quit_chord_on_a_preserved_dirty_doc_still_quits() {
        let mut app = app();
        // Genuinely dirty (a content comparison, not the cache) —
        // `is_dirty_now`'s re-derive would just overwrite a cache poke.
        app.doc_mut(app.active)
            .expect("active doc exists")
            .saved_content = Arc::from("");
        app.doc_mut(app.active).expect("active doc exists").replica = Replica::Bound(
            crate::db::DocDb::new(1, crate::db::PublishMode::CreateOnly, rune_db::Seq(0)),
        );
        app.db = Some(live_db());

        handle_quit_key(&mut app, QuitKey::CtrlC);
        assert!(!app.should_quit, "the first press only arms the confirm");
        handle_quit_key(&mut app, QuitKey::CtrlC);
        assert!(app.should_quit, "the second matching press quits");
    }

    /// The quit chord, resolved through `App::update`'s real `Msg::Key`
    /// dispatch, never reaches `handle_quit_key` at all while a Guard is
    /// already showing (`dispatch::handle_key`'s Stage 1 routes every key
    /// to the existing prompt first) — so a foreign Guard already up is
    /// exercised by calling `handle_quit_key` directly, exactly the real
    /// entry point `Command::QuitConfirm` resolves to; the two paths only
    /// ever differ by that already-showing-prompt short circuit, never by
    /// what `handle_quit_key` itself does. A DirtyQuit raise attempt against
    /// an occupied slot must warn and leave the original prompt alone,
    /// rather than silently dropping the quit intent.
    #[test]
    fn quit_chord_while_a_different_guard_is_up_warns_and_preserves_it() {
        let mut app = app();
        app.doc_mut(app.active)
            .expect("active doc exists")
            .saved_content = Arc::from("");
        assert!(
            !app.active_doc().is_store_bound(),
            "test setup: no db binding"
        );
        let other_doc = app.active;
        assert_eq!(
            crate::guard::set_guard(
                &mut app,
                GuardPrompt {
                    doc: other_doc,
                    kind: GuardKind::DiskConflict,
                },
            ),
            crate::guard::GuardRaise::Raised,
            "test setup: pre-arm a foreign guard"
        );

        handle_quit_key(&mut app, QuitKey::CtrlC);

        assert!(!app.should_quit);
        assert!(
            matches!(
                app.guard,
                Some(GuardPrompt {
                    kind: GuardKind::DiskConflict,
                    ..
                })
            ),
            "the pre-existing prompt must survive unchanged"
        );
        assert_eq!(
            crate::messages::newest_text(&app),
            Some("quit confirmation dropped \u{2014} a prompt is already showing")
        );
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
            crate::search::open(&mut app);
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
        app.frame_width = 120;
        app.frame_height = 34;
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
        app.frame_width = 120;
        app.frame_height = 34;
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
            app.frame_width = 120;
            app.frame_height = 34;
            app.active_doc_mut().saved_content = Arc::from("");
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
        app.frame_width = 120;
        app.frame_height = 34;
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
