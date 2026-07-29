//! The sampled display-pipeline checkers (`SYNC-IDEMPOTENT`, `WRAP-RT`) and
//! the end-of-session undo/redo drive, split out of `driver` (§1.6 budget).
//! Nothing here changes behaviour — every function is exactly what `driver`
//! used to define locally, now reached through `checks::`.

use rune_tui::app::App;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::pane::Pane;
use rune_tui::render;

use crate::invariant::{self, Violation};
use crate::snapshot::Snapshot;

use super::{Outcome, State, key_step, step_and_check};

/// `SYNC-IDEMPOTENT`/`CELL-*`/`WRAP-RT` sampling cadence (G19: the display
/// pipeline — comrak parse -> emit -> wrap — runs on every `sync_view()`,
/// dominating debug-build runtime): full check for the first 32 steps,
/// then every 8th, mirroring Go's precedent.
pub(super) fn should_sample(step: usize) -> bool {
    step <= 32 || step.is_multiple_of(8)
}

/// Builds the current visible rows, or an empty grid before the first
/// sync — mirrors `Snapshot::capture`'s own `cells` derivation.
fn build_rows_or_empty(app: &App) -> Vec<Vec<render::Cell>> {
    match &app.active_doc().view {
        Some(view) => render::build_rows(view, app),
        None => Vec::new(),
    }
}

/// `SYNC-IDEMPOTENT` (G6: `sync_view()` is a genuine fixpoint — `Document::
/// view` never reads `viewport.scroll_row`, and `Viewport::reconcile`
/// converges in one call, plan WP7.S1). Calls `app.sync_view()` a SECOND
/// time with no intervening message and compares the rendered rows and
/// scroll position against the state just before that second call; a
/// divergence is a real non-settling scroll or a non-memoized parse, never
/// a false positive.
pub(super) fn sync_idempotent_check(app: &mut App) -> Option<Violation> {
    let scroll_before = app.active_doc().viewport.scroll_row;
    let rows_before = build_rows_or_empty(app);
    app.sync_view();
    let scroll_after = app.active_doc().viewport.scroll_row;
    let rows_after = build_rows_or_empty(app);
    invariant::sync_idempotent(&rows_before, scroll_before, &rows_after, scroll_after)
}

/// `WRAP-RT` (G7): the forward composition `wrap_to_syntax(syntax_to_wrap(
/// ..))` is an identity over the exact in-domain rectangle
/// `invariant::wrap_line_lens` computes from the CURRENT `ViewSnapshots.
/// wrap` — never a fixed/stale bound, so this can never false-positive
/// against a legitimately re-wrapped document.
pub(super) fn wrap_rt_check(app: &App, line_count: usize) -> Option<Violation> {
    let view = app.active_doc().view.as_ref()?;
    let content = app.active_doc().buffer.content();
    let line_lens = invariant::wrap_line_lens(&view.wrap, line_count);
    invariant::wrap_rt(content, &view.wrap, &line_lens)
}

/// Hands the keyboard back to the editor before the end-of-session
/// undo/redo drive begins, using the same keys a user would press — never
/// by poking `App` directly. `⌘Z` reaching the document is a PRECONDITION
/// `UNDO-TOTAL`/`REDO-TOTAL` need, not a property they assert: per-pane
/// routing (plan Context, decision 8) means an unfocused editor correctly
/// ignores `⌘Z` (only `Editor`'s own keymap binds `Command::Undo`), and a modal
/// correctly captures every key at stage 1 before any pane sees it.
/// Both are reachable at
/// session end today: `^x` (`ToggleExplorer`) leaves the Explorer
/// focused, and an Explorer `Enter` on a path missing from the fuzz `Mem`
/// raises `Modal::Error`. Each press runs through
/// `step_and_check`, so every per-step invariant still applies and a
/// violation here still stops the session, same as any other step.
///
/// Order: `Escape` first, only while a modal is up — both `Modal::Error`
/// and `Modal::Guard` clear on it without touching a buffer byte
/// Then `^E` (`GlobalCommand::FocusEditor`), only
/// while focus isn't already `Pane::Editor` — re-checked
/// fresh rather than decided up front, since dismissing the modal can
/// itself leave focus somewhere other than `Editor`.
fn restore_editor_focus(state: &mut State, prev: &mut Snapshot, outcome: &mut Outcome) -> bool {
    if state.app.modal.is_some() {
        let (msg, tag) = key_step(KeyInput {
            code: KeyCode::Escape,
            mods: Mods::NONE,
        });
        if step_and_check(state, prev, msg, tag, None, outcome) {
            return true;
        }
    }
    if state.app.focus != Pane::Editor {
        let (msg, tag) = key_step(KeyInput {
            code: KeyCode::Char('e'),
            mods: Mods {
                ctrl: true,
                ..Mods::NONE
            },
        });
        if step_and_check(state, prev, msg, tag, None, outcome) {
            return true;
        }
    }
    false
}

/// WP6.S4 end-of-session checks (once): drive `sup+z` down to
/// `journal_pos == 0` (`UNDO-TOTAL`, content-only per G5), then
/// `sup+shift+z` back up to the journal_pos the session was ACTUALLY at
/// when this drive began (`REDO-TOTAL`) — not unconditionally
/// `journal_len`: the session's own last action(s) can legitimately be
/// an undo, leaving an intact, never-superseded redo tail past the
/// point the session stopped at (`invariant::redo_total`'s docs).
/// Skipped once a violation already stopped the session, or the session
/// tore itself down via quit (G15: a torn-down model must not receive
/// more input, same as Go's driver).
pub(super) fn drive_end_of_session_checks(
    state: &mut State,
    prev: &mut Snapshot,
    outcome: &mut Outcome,
    content: &str,
) {
    if outcome.violation.is_none()
        && !state.app.should_quit
        && !restore_editor_focus(state, prev, outcome)
    {
        let pre_undo = prev.clone();
        let bound = pre_undo.journal_len.saturating_add(8);

        let mut presses = 0usize;
        while outcome.violation.is_none()
            && !state.app.should_quit
            && prev.journal_pos != 0
            && presses < bound
        {
            let (msg, tag) = key_step(KeyInput {
                code: KeyCode::Char('z'),
                mods: Mods {
                    sup: true,
                    ..Mods::NONE
                },
            });
            if step_and_check(state, prev, msg, tag, None, outcome) {
                break;
            }
            presses += 1;
        }
        if outcome.violation.is_none()
            && let Some(v) = invariant::undo_total(content, prev)
        {
            outcome.violation = Some(v);
            outcome.final_snapshot = Some(Snapshot::capture(&mut state.app, true));
        }

        if outcome.violation.is_none() && !state.app.should_quit {
            let mut redo_presses = 0usize;
            while outcome.violation.is_none()
                && !state.app.should_quit
                && prev.journal_pos != pre_undo.journal_pos
                && redo_presses < bound
            {
                let (msg, tag) = key_step(KeyInput {
                    code: KeyCode::Char('z'),
                    mods: Mods {
                        sup: true,
                        shift: true,
                        ..Mods::NONE
                    },
                });
                if step_and_check(state, prev, msg, tag, None, outcome) {
                    break;
                }
                redo_presses += 1;
            }
            if outcome.violation.is_none()
                && let Some(v) = invariant::redo_total(&pre_undo, prev)
            {
                outcome.violation = Some(v);
                outcome.final_snapshot = Some(Snapshot::capture(&mut state.app, true));
            }
        }
    }
}
