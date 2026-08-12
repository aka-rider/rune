//! The sampled display-pipeline checkers (`SYNC-IDEMPOTENT`, `WRAP-RT`) and
//! the end-of-session undo/redo drive, split out of `driver` (500-line
//! budget). Nothing here changes behaviour — every function is
//! exactly what `driver` used to define locally, now reached through
//! `checks::`.

use rune_tui::app::App;
use rune_tui::document::ReadOnly;
use rune_tui::focus::{self, FocusTarget};
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::pane::Pane;
use rune_tui::render;

use crate::invariant::{self, Violation};
use crate::snapshot::Snapshot;

use super::{Outcome, State, key_step, step_and_check};

/// `SYNC-IDEMPOTENT`/`CELL-*`/`WRAP-RT` sampling cadence (G19: the display
/// pipeline — comrak parse -> emit -> wrap — runs on every `sync_view()`,
/// dominating debug-build runtime): full check for the first 32 steps,
/// then every 8th.
pub(super) fn should_sample(step: usize) -> bool {
    step <= 32 || step.is_multiple_of(8)
}

/// Builds the current visible rows, or an empty grid before the first
/// sync — mirrors `Snapshot::capture`'s own `cells` derivation.
fn build_rows_or_empty(app: &App) -> Vec<Vec<render::Cell>> {
    match &app.active_doc().view {
        Some(view) => render::build_rows(app, app.active_doc(), Some(app.active), view),
        None => Vec::new(),
    }
}

/// `SYNC-IDEMPOTENT` (G6: `sync_view()` is a genuine fixpoint — `Document::
/// view` never reads `viewport.scroll_row`, and `Viewport::reconcile`
/// converges in one call). Two independent halves, both
/// against the SAME already-synced state (nothing between them mutates the
/// document):
///
/// 1. Display-pipeline idempotence, bypassing the `dirty`-flag memo via
///    `DocMachine::force_rebuild`: a naive second `app.sync_view()` call
///    would just hit the memo and trivially equal the first render
///    regardless of whether the underlying pipeline is actually a
///    fixpoint — so this compares the cached production render against a
///    genuinely-rebuilt one instead.
/// 2. Scroll idempotence: a real second, message-free `app.sync_view()`
///    call must not move `scroll_row` — `Viewport::reconcile`'s own
///    fixpoint claim, independent of whatever the display pipeline memo
///    does.
pub(super) fn sync_idempotent_check(app: &mut App) -> Option<Violation> {
    crate::fault::fire_before_sync_idempotent_check();
    let production_rows = build_rows_or_empty(app);
    let rebuilt_rows = {
        let doc = app.active_doc();
        let forced = doc.doc.force_rebuild(&doc.buffer);
        render::build_rows(app, app.active_doc(), Some(app.active), &forced)
    };
    if let Some(v) = invariant::sync_idempotent_rebuild(&production_rows, &rebuilt_rows) {
        return Some(v);
    }

    let scroll_before = app.active_doc().viewport.scroll_row.0;
    app.sync_view();
    let scroll_after = app.active_doc().viewport.scroll_row.0;
    let rows_after = build_rows_or_empty(app);
    invariant::sync_idempotent(&production_rows, scroll_before, &rows_after, scroll_after)
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

/// The `^1`-`^0` chord that jumps straight to the tab at `idx`
/// (`GlobalCommand::TabSwitch`) — `^0` names the TENTH tab, matching the
/// digit the tab strip itself prints. `None` past the tenth tab, which no
/// chord names at all.
fn tab_switch_key(idx: usize) -> Option<KeyInput> {
    const DIGITS: [char; 10] = ['1', '2', '3', '4', '5', '6', '7', '8', '9', '0'];
    Some(KeyInput {
        code: KeyCode::Char(*DIGITS.get(idx)?),
        mods: Mods {
            ctrl: true,
            ..Mods::NONE
        },
    })
}

/// Hands the keyboard back to the editor before the end-of-session
/// undo/redo drive begins, using the same keys a user would press — never
/// by poking `App` directly. `⌘Z` reaching the SEEDED document (not just
/// `Pane::Editor`) is a PRECONDITION `UNDO-TOTAL`/`REDO-TOTAL` need, not a
/// property they assert: per-pane routing means
/// an unfocused editor correctly ignores `⌘Z` (only `Editor`'s own keymap
/// binds `Command::Undo`), and a modal correctly captures every key at
/// stage 1 before any pane sees it. Four preconditions are reachable at
/// session end today: `^b` (`GlobalCommand::FocusExplorer`) leaves the
/// Explorer focused — `^x` was retired as the explorer chord once the
/// held-space leader took over as the primary way in — an Explorer
/// `Enter` on a path missing from the fuzz `Mem` posts an error message;
/// `F1` (`GlobalCommand::Help`)
/// switches `app.active` itself to the virtual Help document —
/// `UNDO-TOTAL`/`REDO-TOTAL` compare against the ORIGINAL seed content,
/// which the Help document can never match (it isn't even the same
/// document, let alone journaled the seed's edits); and a Tabs-pane
/// `Enter` (`TabsCommand::Select`) activates whatever tab its cursor sits
/// on, which is just as likely to be the untitled draft `App::new` mints
/// before the seed is ever opened — a real, editable, journaled document
/// whose own undo converges perfectly well to an origin content that is
/// simply not the seed's. Each press runs
/// through `step_and_check`, so every per-step invariant still applies
/// and a violation here still stops the session, same as any other step.
///
/// Order: `Escape` first, only while a Guard is up — it clears on `Escape`
/// without touching a buffer byte. Then
/// `F1` again, only while the Help document is active — `workspace::
/// toggle_help`'s own docs say this switches back to whatever was active
/// right before Help was last activated, which is NOT necessarily the
/// seeded document: `App::new` mints an untitled draft before the seed is
/// ever opened and `⌘N` can mint further ones, so a session always has at
/// least two non-Help documents this could land on. Pinning the seed as
/// active is the tab-switch step's job below; this press exists only to
/// leave the Help document, which binds no editing key at all. Then, only
/// while focus is `Pane::Title`,
/// plain `Escape` — NEVER `^B` there, and deliberately BEFORE
/// the generic `^B` branch below: `^B` (`GlobalCommand::ToggleLeft`) is a
/// TOGGLE, and pressing it while the title is focused would (after the
/// hoisted blur every `GlobalCommand` runs first) show or hide the column
/// rather than doing anything useful for the title itself; `Escape` is the
/// title's own dedicated exit. `Escape` has no failure mode `^B` would:
/// `title::keys::handle_key`'s own `Escape` arm reverts the field to
/// `committed` FIRST, so `on_blur`'s `text == committed` check trivially
/// holds and the blur can never be `Refused` — "Escape is always an
/// exit" is exactly this guarantee, reused here as the driver's
/// own unconditional way out of the title. Finally `^B`, only while focus
/// still isn't `Pane::Editor` — re-checked fresh rather than decided up
/// front, since dismissing the modal, toggling Help off, or leaving the
/// title can each independently land focus somewhere other than `Editor`
/// (Explorer, Tabs). Both of those panes can only ever hold focus while the
/// left column is painted (`LayoutMode::focusable`'s own invariant), so
/// `^B`'s hide branch is guaranteed to reach the Editor from either one —
/// unlike a plain `Escape` there, which is unbound in both panes' own key
/// tables and would do nothing. Finally, only once focus is back on
/// `Editor`, plain `Escape` again while merge mode is `Active` —
/// `merge::keys::intercept` owns every key on its document at that point,
/// so leaving it Active into the sweep below would spend the whole `⌘Z`
/// budget being swallowed by the resolver's own feedback fallback instead
/// of ever reaching the journal.
fn restore_editor_focus(state: &mut State, prev: &mut Snapshot, outcome: &mut Outcome) -> bool {
    if state.app.guard.is_some() {
        let (msg, tag) = key_step(KeyInput {
            code: KeyCode::Escape,
            mods: Mods::NONE,
        });
        if step_and_check(state, prev, msg, tag, None, outcome) {
            return true;
        }
    }
    if state.app.help_doc == Some(state.app.active) {
        let (msg, tag) = key_step(KeyInput {
            code: KeyCode::F1,
            mods: Mods::NONE,
        });
        if step_and_check(state, prev, msg, tag, None, outcome) {
            return true;
        }
    }
    if state.app.focus() == Pane::Title {
        let (msg, tag) = key_step(KeyInput {
            code: KeyCode::Escape,
            mods: Mods::NONE,
        });
        if step_and_check(state, prev, msg, tag, None, outcome) {
            return true;
        }
    }
    // The search bar isn't a `Pane` at all (it's its own focus state,
    // checked by `focus::target` ahead of the chrome-level `Pane` match),
    // so the generic `^B` fallback below — which reads `app.focus()` —
    // could never reach it either way. Esc is the bar's own dedicated
    // exit and closes it outright (`search::keys::handle_key`'s `Escape`
    // arm), mirroring the Title case just above.
    if focus::target(&state.app) == FocusTarget::SearchField {
        let (msg, tag) = key_step(KeyInput {
            code: KeyCode::Escape,
            mods: Mods::NONE,
        });
        if step_and_check(state, prev, msg, tag, None, outcome) {
            return true;
        }
    }
    // The message pane isn't gated behind the left column, so the generic
    // `^B` fallback below can't reach it — `^B` toggles a column this pane
    // doesn't live in, leaving focus stuck here forever.
    // `Escape` is its own dedicated exit (`messages::handle_key`), mirroring
    // the Title case just above.
    if state.app.focus() == Pane::Messages {
        let (msg, tag) = key_step(KeyInput {
            code: KeyCode::Escape,
            mods: Mods::NONE,
        });
        if step_and_check(state, prev, msg, tag, None, outcome) {
            return true;
        }
    }
    // `⌘Z` reaches whichever document is ACTIVE, so the seed merely being
    // open is not the precondition — it has to be the one under the
    // keyboard. A session's own keys can leave another one there: a Tabs
    // `Enter` on the untitled draft `App::new` mints before the seed is
    // opened, a `⌘N` draft, an Explorer selection. `^1`-`^0` is the one
    // chord that names a tab positionally from any pane, and it lands focus
    // on the Editor itself, so it doubles as the restore below. Driven
    // BEFORE the merge and reading-view restores, both of which describe
    // the active document and would otherwise be read off the wrong one.
    if state.app.active != state.seed_doc
        && let Some(idx) = state
            .app
            .documents
            .order()
            .iter()
            .position(|&t| t == state.seed_doc)
        && let Some(key) = tab_switch_key(idx)
    {
        let (msg, tag) = key_step(key);
        if step_and_check(state, prev, msg, tag, None, outcome) {
            return true;
        }
    }
    if state.app.focus() != Pane::Editor {
        let (msg, tag) = key_step(KeyInput {
            code: KeyCode::Char('b'),
            mods: Mods {
                ctrl: true,
                ..Mods::NONE
            },
        });
        if step_and_check(state, prev, msg, tag, None, outcome) {
            return true;
        }
    }
    // The merge resolver owns every key on its document while `Active`
    // (`merge::keys::intercept`'s own docs) — the undo sweep below would
    // otherwise spend its whole `⌘Z` budget being consumed by the
    // resolver instead of ever reaching the journal. `Escape` is its own
    // dedicated exit (`MergeCommand::Exit` -> `merge::exit_in_place`),
    // reached only once the focus restores above have already landed back
    // on `Pane::Editor` — the resolver's intercept is scoped to that pane.
    if matches!(state.app.merge, rune_tui::merge::MergeState::Active { .. }) {
        let (msg, tag) = key_step(KeyInput {
            code: KeyCode::Escape,
            mods: Mods::NONE,
        });
        if step_and_check(state, prev, msg, tag, None, outcome) {
            return true;
        }
    }
    // Reading view blocks undo/redo by design, so a session that ends in it
    // would leave the sweep below pressing keys that are correctly ignored —
    // the same "keys went nowhere" misreading the focus restores above
    // exist to prevent. `^p` leaves it. Restoring the precondition here
    // rather than teaching `undo_total`/`redo_total` to skip a read-only
    // document keeps those invariants asserted on EVERY session: a document
    // in reading view can still converge, it just has to leave the view
    // first. Only `ReadOnly::Reading` is restorable — a document with no
    // editable form at all refuses the toggle, and the sweep is already
    // gated on the seeded document still being present.
    if state.app.active_doc().read_only == ReadOnly::Reading {
        let (msg, tag) = key_step(KeyInput {
            code: KeyCode::Char('p'),
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

/// End-of-session checks (once): drive `sup+z` down to
/// `journal_pos == 0` (`UNDO-TOTAL`, content-only per G5), then
/// `sup+shift+z` back up to the journal_pos the session was ACTUALLY at
/// when this drive began (`REDO-TOTAL`) — not unconditionally
/// `journal_len`: the session's own last action(s) can legitimately be
/// an undo, leaving an intact, never-superseded redo tail past the
/// point the session stopped at (`invariant::redo_total`'s docs).
/// Skipped once a violation already stopped the session, or the session
/// tore itself down via quit (G15: a torn-down model must not receive
/// more input).
///
/// Also skipped whenever the drive would not be running on the SEEDED
/// document. Both checkers compare against the content THIS session was
/// seeded with, while `⌘Z` reaches whichever document is active — run them
/// on any other one and they measure real convergence against the wrong
/// operand: an untitled draft undone to `journal_pos == 0` is legitimately
/// empty, and reporting that as a failed `UNDO-TOTAL` says nothing about
/// undo at all. `restore_editor_focus` drives the seed back to active
/// first; this gate is what makes the guarantee hold unconditionally,
/// whatever keys the session ended on and whatever document they left
/// active.
///
/// The seed can also be gone outright before any of this runs — a
/// quit-chord's dirty-close Guard, armed on a document other than the one
/// currently active, legitimately discards it via `[D]iscard`, production
/// working exactly as designed (the per-document dirty gate has no "but
/// it's not the active one" exception). A discarded document has no undo
/// history left to prove anything about and no tab to switch back to, so
/// that case leaves before spending a single restore keystroke on it.
pub(super) fn drive_end_of_session_checks(
    state: &mut State,
    prev: &mut Snapshot,
    outcome: &mut Outcome,
    content: &str,
) {
    if outcome.violation.is_none()
        && !state.app.should_quit
        && state.app.documents.contains_key(&state.seed_doc)
        && !restore_editor_focus(state, prev, outcome)
        && state.app.active == state.seed_doc
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
