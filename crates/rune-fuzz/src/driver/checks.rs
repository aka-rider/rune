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
/// converges in one call, plan WP7.S1). Two independent halves, both
/// against the SAME already-synced state (nothing between them mutates the
/// document):
///
/// 1. Display-pipeline idempotence, bypassing WP16's `dirty`-flag memo via
///    `DocMachine::force_rebuild`: a naive second `app.sync_view()` call
///    would just hit the memo and trivially equal the first render
///    regardless of whether the underlying pipeline is actually a
///    fixpoint (CODE-REVIEW.md rune-fuzz finding 1) — so this compares the
///    cached production render against a genuinely-rebuilt one instead.
/// 2. Scroll idempotence: a real second, message-free `app.sync_view()`
///    call must not move `scroll_row` — `Viewport::reconcile`'s own
///    fixpoint claim, independent of whatever the display pipeline memo
///    does.
pub(super) fn sync_idempotent_check(app: &mut App) -> Option<Violation> {
    let production_rows = build_rows_or_empty(app);
    let rebuilt_rows = {
        let doc = app.active_doc();
        let forced = doc.doc.force_rebuild(&doc.buffer);
        render::build_rows(&forced, app)
    };
    if let Some(v) = invariant::sync_idempotent_rebuild(&production_rows, &rebuilt_rows) {
        return Some(v);
    }

    let scroll_before = app.active_doc().viewport.scroll_row;
    app.sync_view();
    let scroll_after = app.active_doc().viewport.scroll_row;
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

/// Hands the keyboard back to the editor before the end-of-session
/// undo/redo drive begins, using the same keys a user would press — never
/// by poking `App` directly. `⌘Z` reaching the SEEDED document (not just
/// `Pane::Editor`) is a PRECONDITION `UNDO-TOTAL`/`REDO-TOTAL` need, not a
/// property they assert: per-pane routing (plan Context, decision 8) means
/// an unfocused editor correctly ignores `⌘Z` (only `Editor`'s own keymap
/// binds `Command::Undo`), and a modal correctly captures every key at
/// stage 1 before any pane sees it. Three preconditions are reachable at
/// session end today: `^b` (`GlobalCommand::FocusExplorer`) leaves the
/// Explorer focused — `^x` was retired as the explorer chord once the
/// held-space leader took over as the primary way in — an Explorer
/// `Enter` on a path missing from the fuzz `Mem` raises `Modal::Error`; and
/// `F1` (`GlobalCommand::Help`, CODE-REVIEW.md rune-fuzz finding 9's fix)
/// switches `app.active` itself to the virtual Help document —
/// `UNDO-TOTAL`/`REDO-TOTAL` compare against the ORIGINAL seed content,
/// which the Help document can never match (it isn't even the same
/// document, let alone journaled the seed's edits). Each press runs
/// through `step_and_check`, so every per-step invariant still applies
/// and a violation here still stops the session, same as any other step.
///
/// Order: `Escape` first, only while a modal is up — both `Modal::Error`
/// and `Modal::Guard` clear on it without touching a buffer byte. Then
/// `F1` again, only while the Help document is active — `workspace::
/// toggle_help`'s own docs say this switches back to whatever was active
/// right before Help was last activated, ORDINARILY the seeded document
/// (this driver never OPENS more than one non-Help document). That is not
/// an absolute guarantee: a quit-chord's dirty-close Guard, armed on a
/// document other than the one currently active, can discard the seeded
/// document entirely (`[D]iscard`) before this runs — `toggle_help`'s own
/// fallback then has nowhere else to switch to and lands back on Help
/// (`TODO-fuzz-undo-total-dirty-close-discard.md`, fixed: the caller,
/// `drive_end_of_session_checks`, now checks `seed_doc` still exists BEFORE
/// calling this function at all, so that case never reaches this `F1`
/// press in the first place). Then, only while focus is `Pane::Title`,
/// plain `Escape` (plan WP5) — NEVER `^E` there, and deliberately BEFORE
/// the generic `^E` branch below: `^E` (`GlobalCommand::FocusEditor`)
/// routes through `App::set_focus`, the SAME blur chokepoint any other
/// focus change does, which `title::on_blur` can legitimately `Refused`
/// (an invalid — e.g. emptied by an unlocked-gate `⌘X` — or a genuinely
/// in-flight-renaming typed name, decision 7) and leave focus stuck on
/// the title forever, silently misdirecting every later `⌘Z`/`⌘⇧Z` press
/// into the TITLE's own unjournaled field instead of the document
/// (`UNDO-TOTAL`/`REDO-TOTAL`'s actual precondition). `Escape` has no such
/// failure mode: `title::keys::handle_key`'s own `Escape` arm reverts the
/// field to `committed` FIRST, so `on_blur`'s `text == committed` check
/// trivially holds and the blur can never be refused — decision 8's "Escape
/// is always an exit" is exactly this guarantee, reused here as the
/// driver's own unconditional way out of the title. Finally `^E`, only
/// while focus still isn't `Pane::Editor` — re-checked fresh rather than
/// decided up front, since dismissing the modal, toggling Help off, or
/// leaving the title can each independently land focus somewhere other
/// than `Editor` (Explorer, Tabs) that only `^E` reaches.
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
    if state.app.focus() != Pane::Editor {
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
/// more input, same as Go's driver). Also skipped once the seeded document
/// itself no longer exists (`TODO-fuzz-undo-total-dirty-close-discard.md`):
/// a quit-chord's dirty-close Guard, armed on a document other than the one
/// currently active, can legitimately discard the seed via its own
/// `[D]iscard` key — production working exactly as designed (§1.4.4's
/// per-document dirty gate has no "but it's not the active one" exception).
/// A discarded document has no undo history left to prove anything about;
/// driving `restore_editor_focus`'s `F1` press in that state would only
/// land back on Help (its own fallback has nothing else to switch to), and
/// `UNDO-TOTAL`/`REDO-TOTAL` would then be comparing the seed against a
/// document that was never the seed to begin with. This is the driver's
/// own precondition to maintain, not a relaxation of either checker.
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
