//! The deterministic engine: drives the real `rune_tui::app::update` against
//! an in-memory `Vfs`, with no terminal, no clock, and no subprocess.
//! Modelled on Go's drain loop (`internal/fuzz/driver/driver.go`), scoped to
//! the seam this crate actually has: `App::new` + `app::update` + `Cmd`
//! (WP2's tagged struct) + `Mem`.
//!
//! This driver never delivers `Msg::Error` or `Msg::Quit`: neither is ever
//! constructed by an `Action` this crate generates, and production itself
//! only ever sends them from paths this driver doesn't exercise (a real
//! terminal input stream ending, or a spawned `Cmd`'s caught panic — see
//! `runtime.rs`). Every `Msg` the driver DOES deliver is tagged with an
//! owned `MsgTag` at the point it's constructed (`crate::step`), so there is
//! no need for a totalizing `Msg -> MsgTag` conversion that would have to
//! account for those two unreachable-here variants.

use std::io;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_tui::app::{self, App};
use rune_tui::keymap::{self, KeyCode, KeyInput, Mods};
use rune_tui::render::{self, Cell};
use rune_tui::runtime::{Cmd, CmdKind, Effects, Msg};
use rune_vfs::{Mem, Vfs};

use crate::action::Action;
use crate::invariant::{self, Violation};
use crate::snapshot::Snapshot;
use crate::step::{MsgTag, StepCtx};

/// The one seeded file path every session opens. Seeded on `Mem` directly
/// at bootstrap (WP3.S7 rule 1), so a save-less session still reads back
/// its starting content via `Vfs::read`.
const DOC_PATH: &str = "/fuzz/doc.md";

/// The result of driving one whole session. `final_snapshot`/`final_ctx`
/// are frozen at the violating step (`None` on a clean run) — Go's driver
/// freezes `rs.frozenFrame`/`frozenCells` the same way
/// (`internal/fuzz/driver/driver.go:190-192`).
pub struct RunResult {
    pub violation: Option<Violation>,
    pub steps: usize,
    pub final_content: String,
    pub final_snapshot: Option<Snapshot>,
    pub final_ctx: Option<StepCtx>,
}

/// Mutable driver state threaded through one session. `pending_save` is an
/// `Option`, never a queue — G9 proves at most one save `Cmd` can ever be
/// outstanding.
struct State {
    app: App,
    mem: Arc<Mem>,
    pending_save: Option<(Cmd, Vec<u8>)>,
    saves_delivered_ok: usize,
    steps: usize,
}

/// Accumulates the frozen state once a violation fires, so the driving loop
/// can stop at the first one (first-wins, WP3.S7 rule 5).
struct Outcome {
    violation: Option<Violation>,
    final_snapshot: Option<Snapshot>,
    final_ctx: Option<StepCtx>,
}

/// Runs `actions` against a fresh session seeded with `content` at
/// `/fuzz/doc.md`. Deterministic: same input, same result, always — zero
/// wall-clock reads, zero threads, zero subprocesses (WP3.S7 rule 7).
pub fn run(content: &str, actions: &[Action]) -> RunResult {
    let mem = Arc::new(Mem::new());
    let _ = mem.save_atomic(Path::new(DOC_PATH), content.as_bytes());
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(&mem) as Arc<dyn Vfs + Send + Sync>;

    let mut app = App::new(
        Buffer::new(content),
        Some(PathBuf::from(DOC_PATH)),
        vfs,
        None,
    );
    app.active_doc_mut().focused = true;
    app.active_doc_mut().viewport.set_size(80, 23);
    app.sync_view();

    let mut state = State {
        app,
        mem,
        pending_save: None,
        saves_delivered_ok: 0,
        steps: 0,
    };
    let mut prev = Snapshot::capture(&mut state.app, false);
    let mut outcome = Outcome {
        violation: None,
        final_snapshot: None,
        final_ctx: None,
    };

    'session: for action in actions {
        if state.app.should_quit {
            break;
        }
        match action {
            Action::FailNextSave => {
                state.mem.fail_next_save(io::ErrorKind::PermissionDenied);
            }
            Action::ConfirmTimeout => {
                if let Some((_, generation)) = state.app.pending_quit {
                    let msg = Msg::ConfirmTimeout { generation };
                    let tag = MsgTag::ConfirmTimeout { generation };
                    if step_and_check(&mut state, &mut prev, msg, tag, None, &mut outcome) {
                        break 'session;
                    }
                }
            }
            Action::Deliver => {
                if let Some((msg, tag, bytes)) = discharge_pending_save(&mut state)
                    && step_and_check(&mut state, &mut prev, msg, tag, Some(bytes), &mut outcome)
                {
                    break 'session;
                }
            }
            Action::Key(k) => {
                let (msg, tag) = key_step(*k);
                if step_and_check(&mut state, &mut prev, msg, tag, None, &mut outcome) {
                    break 'session;
                }
            }
            Action::Paste(s) => {
                let tag = MsgTag::Paste(s.clone());
                if step_and_check(
                    &mut state,
                    &mut prev,
                    Msg::Paste(s.clone()),
                    tag,
                    None,
                    &mut outcome,
                ) {
                    break 'session;
                }
            }
            Action::Resize(w, h) => {
                let tag = MsgTag::Resize(*w, *h);
                if step_and_check(
                    &mut state,
                    &mut prev,
                    Msg::Resize(*w, *h),
                    tag,
                    None,
                    &mut outcome,
                ) {
                    break 'session;
                }
            }
            Action::ClipboardReply(s) => {
                let tag = MsgTag::ClipboardRead(s.clone());
                if step_and_check(
                    &mut state,
                    &mut prev,
                    Msg::ClipboardRead(s.clone()),
                    tag,
                    None,
                    &mut outcome,
                ) {
                    break 'session;
                }
            }
            Action::Type(s) => {
                for ch in s.chars() {
                    debug_assert!(
                        ch == '\n' || !ch.is_control(),
                        "Action::Type payload contains an undeliverable control char {ch:?}; \
                         the generator must route byte-hostile payloads through Action::Paste \
                         (is_insertable_key_char silently drops it — plan Gotcha G3)"
                    );
                    let (msg, tag) = key_step(KeyInput {
                        code: if ch == '\n' {
                            KeyCode::Enter
                        } else {
                            KeyCode::Char(ch)
                        },
                        mods: Mods::NONE,
                    });
                    if step_and_check(&mut state, &mut prev, msg, tag, None, &mut outcome) {
                        break 'session;
                    }
                    if state.app.should_quit {
                        break;
                    }
                }
            }
        }
    }

    // Rule 6: discharge any still-pending save before finishing, unless a
    // violation already stopped the session or a quit tore the model down.
    if outcome.violation.is_none()
        && !state.app.should_quit
        && let Some((msg, tag, bytes)) = discharge_pending_save(&mut state)
    {
        step_and_check(&mut state, &mut prev, msg, tag, Some(bytes), &mut outcome);
    }

    // WP6.S4 end-of-session checks (once): drive `sup+z` down to
    // `journal_pos == 0` (`UNDO-TOTAL`, content-only per G5), then
    // `sup+shift+z` back up to the journal_pos the session was ACTUALLY at
    // when this drive began (`REDO-TOTAL`) — not unconditionally
    // `journal_len`: the session's own last action(s) can legitimately be
    // an undo, leaving an intact, never-superseded redo tail past the
    // point the session stopped at (`invariant::redo_total`'s docs).
    // Skipped once a violation already stopped the session, or the session
    // tore itself down via quit (G15: a torn-down model must not receive
    // more input, same as Go's driver).
    if outcome.violation.is_none() && !state.app.should_quit {
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
            if step_and_check(&mut state, &mut prev, msg, tag, None, &mut outcome) {
                break;
            }
            presses += 1;
        }
        if outcome.violation.is_none()
            && let Some(v) = invariant::undo_total(content, &prev)
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
                if step_and_check(&mut state, &mut prev, msg, tag, None, &mut outcome) {
                    break;
                }
                redo_presses += 1;
            }
            if outcome.violation.is_none()
                && let Some(v) = invariant::redo_total(&pre_undo, &prev)
            {
                outcome.violation = Some(v);
                outcome.final_snapshot = Some(Snapshot::capture(&mut state.app, true));
            }
        }
    }

    RunResult {
        violation: outcome.violation,
        steps: state.steps,
        final_content: prev.content,
        final_snapshot: outcome.final_snapshot,
        final_ctx: outcome.final_ctx,
    }
}

/// Builds the `(Msg, MsgTag)` pair for one keystroke — the one place
/// `keymap::resolve` is consulted for tagging, shared by `Action::Key` and
/// `Action::Type`'s per-char expansion.
fn key_step(key: KeyInput) -> (Msg, MsgTag) {
    let tag = MsgTag::Key {
        input: key,
        command: keymap::resolve(key),
    };
    (Msg::Key(key), tag)
}

/// Runs the one deferred save `Cmd`, if any, returning the `Msg` it
/// produced together with its tag and the bytes it was constructed with.
/// `save_cmd` (`app.rs`) only ever constructs a `Msg::SaveDone` reply and
/// never returns `None` — the `None` arms below are defensive against
/// `Cmd`'s general contract, not reachable from any real save `Cmd` this
/// driver stores. Never synthesizes `Msg::SaveDone` itself (G14) — it only
/// ever forwards what `cmd.run()` actually returned.
fn discharge_pending_save(state: &mut State) -> Option<(Msg, MsgTag, Vec<u8>)> {
    let (cmd, bytes) = state.pending_save.take()?;
    let msg = cmd.run()?;
    let Msg::SaveDone {
        version, result, ..
    } = &msg
    else {
        return None;
    };
    let tag = MsgTag::SaveDone {
        version: *version,
        ok: result.is_ok(),
    };
    Some((msg, tag, bytes))
}

/// Delivers one message through `update`, captures the resulting `Snapshot`
/// and `StepCtx`, checks every invariant, and records a violation (if any)
/// into `outcome`. Returns `true` when the session must stop.
fn step_and_check(
    state: &mut State,
    prev: &mut Snapshot,
    msg: Msg,
    tag: MsgTag,
    delivered_save_bytes: Option<Vec<u8>>,
    outcome: &mut Outcome,
) -> bool {
    state.steps += 1;
    let step_index = state.steps;
    let is_save_done_ok = matches!(&tag, MsgTag::SaveDone { ok: true, .. });

    let effects = match run_update_catching_panic(&mut state.app, msg) {
        Ok(effects) => effects,
        Err(payload) => {
            outcome.violation = Some(Violation {
                id: "NO-PANIC",
                message: downcast_panic(&payload),
            });
            outcome.final_snapshot = Some(prev.clone());
            outcome.final_ctx = None;
            return true;
        }
    };

    // Classify every Cmd this step produced (WP3.S7 rule 3): Save is
    // deferred (there can only ever be one, G9); QuitTimeout/ClipboardRead
    // are dropped — a headless driver must never sleep 2 real seconds or
    // fork `/usr/bin/pbpaste`.
    for cmd in effects.cmds {
        if cmd.kind() == CmdKind::Save {
            state.pending_save = Some((cmd, prev.content.clone().into_bytes()));
        }
    }

    if is_save_done_ok {
        state.saves_delivered_ok += 1;
    }

    let sampled = should_sample(step_index);
    let next = Snapshot::capture(&mut state.app, sampled);
    let disk = state.mem.read(Path::new(DOC_PATH)).ok();
    let pending_save_bytes = state.pending_save.as_ref().map(|(_, b)| b.clone());

    let ctx = StepCtx {
        step: step_index,
        msg: tag,
        raw: effects.raw,
        disk,
        pending_save_bytes,
        delivered_save_bytes,
        saves_delivered_ok: state.saves_delivered_ok,
    };

    let mut violation = invariant::check_all(prev, &next, &ctx);
    // `SYNC-IDEMPOTENT`/`WRAP-RT` need live `&mut App`/`ViewSnapshots`
    // access `Snapshot` can't carry (module docs) — checked only on
    // sampled steps (G19: the display pipeline dominates debug-build
    // runtime).
    if violation.is_none() && sampled {
        violation = sync_idempotent_check(&mut state.app)
            .or_else(|| wrap_rt_check(&state.app, next.line_count));
    }
    *prev = next;

    if let Some(v) = violation {
        // Capture cells unconditionally on the violating step even though
        // sampling is off for every WP3 checker (G19) — WP5's report needs
        // the rendered frame regardless of which invariant fired.
        outcome.final_snapshot = Some(Snapshot::capture(&mut state.app, true));
        outcome.final_ctx = Some(ctx);
        outcome.violation = Some(v);
        return true;
    }
    false
}

/// Wraps one `update` + `sync_view` call in `catch_unwind` — G13's three
/// live `debug_assert!`s (buffer line-starts, undo `reapply`, render cell-
/// map length) fire in this debug build, and any of them (or a genuine
/// panic anywhere in the pipeline) must produce a `NO-PANIC` violation
/// rather than aborting the whole fuzz run.
fn run_update_catching_panic(
    app: &mut App,
    msg: Msg,
) -> Result<Effects, Box<dyn std::any::Any + Send>> {
    std::panic::catch_unwind(AssertUnwindSafe(move || {
        let mut effects = Effects::default();
        app::update(app, msg, &mut effects);
        app.sync_view();
        effects
    }))
}

/// The same downcast ladder proptest itself uses to render a caught panic's
/// payload (`proptest-1.11.0/src/test_runner/runner.rs:255-264`).
fn downcast_panic(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "<unknown panic value>".to_string()
    }
}

/// `SYNC-IDEMPOTENT`/`CELL-*`/`WRAP-RT` sampling cadence (G19: the display
/// pipeline — comrak parse -> emit -> wrap — runs on every `sync_view()`,
/// dominating debug-build runtime): full check for the first 32 steps,
/// then every 8th, mirroring Go's precedent
/// (`internal/fuzz/driver/driver_v4_properties.go:57`).
fn should_sample(step: usize) -> bool {
    step <= 32 || step.is_multiple_of(8)
}

/// Builds the current visible rows, or an empty grid before the first
/// sync — mirrors `Snapshot::capture`'s own `cells` derivation.
fn build_rows_or_empty(app: &App) -> Vec<Vec<Cell>> {
    match &app.active_doc().view {
        Some(view) => render::build_rows(view, app),
        None => Vec::new(),
    }
}

/// `SYNC-IDEMPOTENT` (G6: `sync_view()` is a genuine fixpoint — `Document::
/// view` never reads `viewport.scroll_row`, and `Viewport::scroll_to_row`
/// converges in one call). Calls `app.sync_view()` a SECOND time with no
/// intervening message and compares the rendered rows and scroll position
/// against the state just before that second call; a divergence is a real
/// non-settling scroll or a non-memoized parse, never a false positive.
fn sync_idempotent_check(app: &mut App) -> Option<Violation> {
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
fn wrap_rt_check(app: &App, line_count: usize) -> Option<Violation> {
    let view = app.active_doc().view.as_ref()?;
    let line_lens = invariant::wrap_line_lens(&view.wrap, line_count);
    invariant::wrap_rt(&view.wrap, &line_lens)
}
