use rune_tui::app::App;
use rune_tui::document::ReadOnly;
use rune_tui::focus::{self, FocusTarget};
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::pane::Pane;
use rune_tui::render;

use crate::invariant::{self, Violation};
use crate::snapshot::Snapshot;

use super::session::{Outcome, State};
use super::step_exec::{key_step, step_and_check};

pub(super) fn should_sample(step: usize) -> bool {
    step <= 32 || step.is_multiple_of(8)
}

fn build_rows_or_empty(app: &App) -> Vec<Vec<render::Cell>> {
    app.shown_doc().view.as_ref().map_or_else(Vec::new, |view| {
        render::build_rows(app, render::RowSource::Shown, view)
    })
}

pub(super) fn sync_idempotent_check(app: &mut App) -> Option<Violation> {
    crate::fault::fire_before_sync_idempotent_check();
    let production_rows = build_rows_or_empty(app);
    let rebuilt_rows = {
        let doc = app.shown_doc();
        let forced = doc.doc.force_rebuild(&doc.buffer);
        render::build_rows(app, render::RowSource::Shown, &forced)
    };
    if let Some(v) = invariant::sync_idempotent_rebuild(&production_rows, &rebuilt_rows) {
        return Some(v);
    }

    let scroll_before = app.shown_doc().viewport.scroll_row.0;
    app.sync_view();
    let scroll_after = app.shown_doc().viewport.scroll_row.0;
    let rows_after = build_rows_or_empty(app);
    invariant::sync_idempotent(&production_rows, scroll_before, &rows_after, scroll_after)
}

pub(super) fn wrap_rt_check(app: &App, line_count: usize) -> Option<Violation> {
    let view = app.active_doc().view.as_ref()?;
    let content = app.active_doc().buffer.content();
    let line_lens = invariant::wrap_line_lens(&view.wrap, line_count);
    invariant::wrap_rt(content, &view.wrap, &line_lens)
}

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

const ESCAPE: KeyInput = KeyInput {
    code: KeyCode::Escape,
    mods: Mods::NONE,
};

fn guard_up_key(state: &State) -> Option<KeyInput> {
    state.app.guard.is_some().then_some(ESCAPE)
}

fn help_active_key(state: &State) -> Option<KeyInput> {
    (state.app.help_doc == Some(state.app.active)).then_some(KeyInput {
        code: KeyCode::F1,
        mods: Mods::NONE,
    })
}

fn title_focused_key(state: &State) -> Option<KeyInput> {
    (state.app.focus() == Pane::Title).then_some(ESCAPE)
}

fn search_field_focused_key(state: &State) -> Option<KeyInput> {
    (focus::target(&state.app) == FocusTarget::SearchField).then_some(ESCAPE)
}

fn palette_focused_key(state: &State) -> Option<KeyInput> {
    (focus::target(&state.app) == FocusTarget::Palette).then_some(ESCAPE)
}

fn filesearch_focused_key(state: &State) -> Option<KeyInput> {
    (focus::target(&state.app) == FocusTarget::FileSearch).then_some(ESCAPE)
}

fn messages_focused_key(state: &State) -> Option<KeyInput> {
    (state.app.focus() == Pane::Messages).then_some(ESCAPE)
}

fn seed_not_active_key(state: &State) -> Option<KeyInput> {
    if state.app.active == state.seed_doc {
        return None;
    }
    let idx = state
        .app
        .documents
        .order()
        .iter()
        .position(|&t| t == state.seed_doc)?;
    tab_switch_key(idx)
}

fn editor_unfocused_key(state: &State) -> Option<KeyInput> {
    (state.app.focus() != Pane::Editor).then_some(KeyInput {
        code: KeyCode::Char('b'),
        mods: Mods {
            ctrl: true,
            ..Mods::NONE
        },
    })
}

fn merge_active_key(state: &State) -> Option<KeyInput> {
    matches!(state.app.merge, rune_tui::merge::MergeState::Active { .. }).then_some(ESCAPE)
}

fn reading_view_key(state: &State) -> Option<KeyInput> {
    (state.app.active_doc().read_only == ReadOnly::Reading).then_some(KeyInput {
        code: KeyCode::Char('P'),
        mods: Mods {
            ctrl: true,
            ..Mods::NONE
        },
    })
}

const RESTORE_STEPS: [fn(&State) -> Option<KeyInput>; 11] = [
    guard_up_key,
    help_active_key,
    title_focused_key,
    search_field_focused_key,
    palette_focused_key,
    filesearch_focused_key,
    messages_focused_key,
    seed_not_active_key,
    editor_unfocused_key,
    merge_active_key,
    reading_view_key,
];

fn restore_editor_focus(state: &mut State, prev: &mut Snapshot, outcome: &mut Outcome) -> bool {
    for _ in 0..RESTORE_STEPS.len() {
        let mut pressed_any = false;
        for step in RESTORE_STEPS {
            let Some(key) = step(state) else {
                continue;
            };
            pressed_any = true;
            let (msg, tag) = key_step(&state.app, key);
            if step_and_check(state, prev, msg, tag, None, outcome) {
                return true;
            }
        }
        if !pressed_any {
            return false;
        }
    }
    false
}

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
        state
            .manual_clock
            .advance(rune_tui::undogroup::LADDER_RESET);

        let pre_undo = prev.clone();
        let bound = pre_undo.journal_len.saturating_add(8);

        let mut presses = 0usize;
        while outcome.violation.is_none()
            && !state.app.should_quit
            && prev.journal_pos != 0
            && presses < bound
        {
            let (msg, tag) = key_step(
                &state.app,
                KeyInput {
                    code: KeyCode::Char('z'),
                    mods: Mods {
                        sup: true,
                        ..Mods::NONE
                    },
                },
            );
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
                let (msg, tag) = key_step(
                    &state.app,
                    KeyInput {
                        code: KeyCode::Char('z'),
                        mods: Mods {
                            sup: true,
                            shift: true,
                            ..Mods::NONE
                        },
                    },
                );
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
