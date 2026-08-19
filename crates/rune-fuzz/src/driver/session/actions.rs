use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::runtime::{Msg, PasteTarget, TimerKey};

use crate::action::Action;
use crate::guard;
use crate::invariant::Violation;
use crate::snapshot::Snapshot;
use crate::step::MsgTag;

use super::super::discharge::{
    discharge_pending_highlight, discharge_pending_rename, discharge_pending_save,
    discharge_pending_trash, drain_one_db_op,
};
use super::super::step_exec::{
    highlight_step, highlight_tree_step, key_step, mouse_step, palette_recents_step, step_and_check,
};
use super::super::store_ops::{diverge_disk, drain_all_db_ops};
use super::{Outcome, State};

/// The fixed root every `Action::DirLoaded` targets (plan WP4.S6) — only
/// `entries`/`cause` vary; the root itself isn't the thing under fuzz here.
const FUZZ_DIR_ROOT: &str = "/fuzz/dir";

pub(super) fn apply(state: &mut State, prev: &mut Snapshot, outcome: &mut Outcome, action: Action) {
    match action {
        Action::FailNextSave => {
            state.mem.fail_next_save(io::ErrorKind::PermissionDenied);
        }
        Action::AdvanceClock(millis) => {
            state
                .manual_clock
                .advance(std::time::Duration::from_millis(millis));
        }
        Action::DivergeDisk => {
            diverge_disk(state, prev, outcome);
        }
        Action::InstallDiffLeft { seed_index } => {
            install_diff_left(state, prev, outcome, seed_index);
        }
        Action::DeliverDb => {
            let bridge = Arc::clone(&state.bridge);
            if let Some((msg, tag)) = drain_one_db_op(state, &bridge) {
                step_and_check(state, prev, msg, tag, None, outcome);
            }
        }
        Action::DeliverDbAll => {
            drain_all_db_ops(state, prev, outcome);
        }
        Action::ConfirmTimeout => {
            if let rune_tui::app::QuitNegotiation::ConfirmArmed(_, generation) = state.app.quit {
                let msg = Msg::Timer {
                    key: TimerKey::QuitConfirm,
                    generation: generation.raw(),
                };
                let tag = MsgTag::ConfirmTimeout {
                    generation: generation.raw() as u32,
                };
                step_and_check(state, prev, msg, tag, None, outcome);
            }
        }
        Action::StaleConfirmTimeout(generation) => {
            // Deliberately no armed-quit precondition (unlike
            // `ConfirmTimeout` above) -- a stale timer firing after
            // `App::quit` already cleared entirely, or after a
            // DIFFERENT generation re-armed it, is exactly the
            // production race this variant exists to exercise.
            let msg = Msg::Timer {
                key: TimerKey::QuitConfirm,
                generation: u64::from(generation),
            };
            let tag = MsgTag::ConfirmTimeout { generation };
            step_and_check(state, prev, msg, tag, None, outcome);
        }
        Action::Deliver => {
            if let Some((msg, tag, bytes)) = discharge_pending_save(state)
                && step_and_check(state, prev, msg, tag, bytes, outcome)
            {
                return;
            }
            if let Some((msg, tag)) = discharge_pending_rename(state)
                && step_and_check(state, prev, msg, tag, None, outcome)
            {
                return;
            }
            if let Some((msg, tag)) = discharge_pending_trash(state)
                && step_and_check(state, prev, msg, tag, None, outcome)
            {
                return;
            }
            if let Some((msg, tag)) = discharge_pending_highlight(state) {
                step_and_check(state, prev, msg, tag, None, outcome);
            }
        }
        Action::Key(k) => {
            let (msg, tag) = key_step(&state.app, k);
            step_and_check(state, prev, msg, tag, None, outcome);
        }
        Action::Mouse(m) => {
            let (msg, tag) = mouse_step(m);
            step_and_check(state, prev, msg, tag, None, outcome);
        }
        Action::OpenFileSearch => {
            let (msg, tag) = key_step(&state.app, crate::action::OPEN_FILESEARCH_KEY);
            step_and_check(state, prev, msg, tag, None, outcome);
        }
        Action::PaletteRecentsLoaded {
            generation,
            ok,
            names,
        } => {
            let (msg, tag) = palette_recents_step(state, generation, ok, names);
            step_and_check(state, prev, msg, tag, None, outcome);
        }
        Action::Paste(s) => {
            let tag = MsgTag::Paste(s.clone());
            step_and_check(state, prev, Msg::Paste(s), tag, None, outcome);
        }
        Action::Resize(w, h) => {
            let tag = MsgTag::Resize(w, h);
            step_and_check(state, prev, Msg::Resize(w, h), tag, None, outcome);
        }
        Action::ClipboardReply(s) => {
            // `PasteTarget::Document(state.app.active)` matches this
            // driver's pre-existing semantics — every `ClipboardReply`
            // this crate synthesizes today lands on whatever document
            // is active (nothing here spawns a title-targeted
            // `pbpaste_cmd`). `MsgTag` now carries the same target so a
            // checker can tell a document-bound reply apart from a
            // title-bound one without reaching into `Msg` itself.
            let target = PasteTarget::Document(state.app.active);
            let tag = MsgTag::ClipboardRead {
                text: s.clone(),
                target,
            };
            let msg = Msg::ClipboardRead { text: s, target };
            step_and_check(state, prev, msg, tag, None, outcome);
        }
        Action::DirLoaded {
            entries,
            cause,
            generation,
        } => {
            let msg = Msg::DirLoaded {
                root: PathBuf::from(FUZZ_DIR_ROOT),
                entries,
                cause,
                generation: rune_tui::generation::Generation::from_raw(u64::from(generation)),
            };
            step_and_check(state, prev, msg, MsgTag::DirLoaded, None, outcome);
        }
        Action::Highlight { version, spans } => {
            let (msg, tag) = highlight_step(state, version, &spans);
            step_and_check(state, prev, msg, tag, None, outcome);
        }
        Action::HighlightTree {
            version,
            fixture,
            base,
        } => {
            let (msg, tag) = highlight_tree_step(state, version, fixture, base);
            step_and_check(state, prev, msg, tag, None, outcome);
        }
        Action::Type(s) => {
            for ch in s.chars() {
                // Demoted (CODE-REVIEW.md rune-fuzz finding 4): this
                // used to be the ONLY thing standing between a
                // control-char `Action::Type` payload and an abort of
                // the whole replay harness — it sits outside
                // `run_update_catching_panic`'s `catch_unwind`, and the
                // script codec happily round-tripped exactly the input
                // that would trip it. Both real sources are closed now:
                // `script::decode`'s `parse_action_line` rejects a
                // control-char `type` payload at decode time (a typed
                // `ScriptError`, never reaching this loop), and every
                // generator draws `Action::Type` payloads only from
                // `TYPE_PALETTE`/`MARKDOWN_FRAGMENTS`, already control-
                // char-free by construction. This is left as
                // defense-in-depth documentation, not a live guard.
                debug_assert!(
                    ch == '\n' || !ch.is_control(),
                    "Action::Type payload contains an undeliverable control char {ch:?}; \
                     the generator must route byte-hostile payloads through Action::Paste \
                     (is_insertable_key_char silently drops it — plan Gotcha G3)"
                );
                let (msg, tag) = key_step(
                    &state.app,
                    KeyInput {
                        code: if ch == '\n' {
                            KeyCode::Enter
                        } else {
                            KeyCode::Char(ch)
                        },
                        mods: Mods::NONE,
                    },
                );
                if step_and_check(state, prev, msg, tag, None, outcome) {
                    return;
                }
                if state.app.should_quit {
                    break;
                }
            }
        }
    }
}

fn install_diff_left(
    state: &mut State,
    prev: &mut Snapshot,
    outcome: &mut Outcome,
    seed_index: u8,
) {
    let left_content = crate::generate::diff_left_content(seed_index);
    let left_name = "fuzz-diff-left.md".to_string();
    let result = guard::catching_panic(|| {
        rune_tui::diff_view::install(&mut state.app, left_content.as_bytes().to_vec(), left_name)
    });
    match result {
        Ok(Ok(())) => {
            *prev = Snapshot::capture(&mut state.app, false);
        }
        Ok(Err(rune_tui::diff_view::DiffInstallError::InvalidUtf8)) => {
            outcome.violation = Some(Violation::new(
                "DIFF-INSTALL-UTF8",
                "diff_view::install refused the generator's own diff-left seed content as \
                 invalid UTF-8"
                    .to_string(),
            ));
            outcome.final_snapshot = Some(prev.clone());
            outcome.final_ctx = None;
        }
        Err(violation) => {
            outcome.violation = Some(violation);
            outcome.final_snapshot = Some(prev.clone());
            outcome.final_ctx = None;
        }
    }
}
