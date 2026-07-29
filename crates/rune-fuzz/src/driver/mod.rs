//! The deterministic engine: drives the real `rune_tui::app::update` against
//! an in-memory `Vfs`, with no terminal, no clock, and no subprocess.
//! Modelled on Go's drain loop, scoped to
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

mod checks;

use std::io;
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_tui::app::{self, App};
use rune_tui::document::DocumentId;
use rune_tui::keymap::{self, KeyCode, KeyInput, Mods};
use rune_tui::runtime::{Cmd, CmdKind, Effects, Msg};
use rune_vfs::{Mem, Vfs};

/// The fixed root every `Action::DirLoaded` targets (plan WP4.S6) — only
/// `entries`/`cause` vary; the root itself isn't the thing under fuzz here.
const FUZZ_DIR_ROOT: &str = "/fuzz/dir";

use crate::action::{Action, HighlightVersion};
use crate::invariant::{self, Violation};
use crate::snapshot::Snapshot;
use crate::step::{MsgTag, StepCtx};

/// The default seeded file path (plan WP7.S2): every `SEEDS` entry
/// inherited from before this package pairs with this path, and the script
/// codec's optional `path` line defaults to it when absent — so the
/// checked-in `repros/tripwire-clean.rune` (written before sessions carried
/// a path) still decodes unchanged.
pub const DOC_PATH: &str = "/fuzz/doc.md";

/// The result of driving one whole session. `final_snapshot`/`final_ctx`
/// are frozen at the violating step (`None` on a clean run) — Go's driver
/// freezes `rs.frozenFrame`/`frozenCells` the same way.
pub struct RunResult {
    pub violation: Option<Violation>,
    pub steps: usize,
    pub final_content: String,
    pub final_snapshot: Option<Snapshot>,
    pub final_ctx: Option<StepCtx>,
}

/// Mutable driver state threaded through one session. `pending_save` is an
/// `Option`, never a queue — G9 proves at most one save `Cmd` can ever be
/// outstanding. Its byte snapshot is keyed by `DocumentId`, not a single
/// `Vec<u8>`: a save `Cmd` can be constructed for a document OTHER than
/// whichever one is active at that instant (a Guard modal's own `s`/`S`
/// hotkey saves ITS prompt's document — `banner::handle_dirty_close_key`
/// — never necessarily `app.active`), so the driver snapshots every open
/// document's content at Cmd-construction time and looks the right one up
/// by `id` once the ack names it (`discharge_pending_save`). `path` is the
/// document path this session opened (plan WP7.S2) — carried here (not
/// re-derived from `DOC_PATH`) since a session can now open any path, and
/// the post-step disk read needs to consult the SAME path the document was
/// seeded and bound to.
struct State {
    app: App,
    mem: Arc<Mem>,
    path: PathBuf,
    pending_save: Option<(Cmd, std::collections::BTreeMap<DocumentId, Vec<u8>>)>,
    saves_delivered_ok: usize,
    steps: usize,
    /// The `DocumentId` `App::new` minted for the seeded document below —
    /// always the very first (and, at that point, only) document, so this
    /// is captured once, before any action runs. `UNDO-TOTAL`/`REDO-TOTAL`
    /// exist to prove undo/redo totality on THIS document; `checks::
    /// drive_end_of_session_checks` consults it to tell "the seed is still
    /// open, just not necessarily active" (F1 toggled to Help — recoverable
    /// by pressing F1 again) apart from "the seed was discarded entirely"
    /// (a dirty-close Guard's `[D]iscard`, per `TODO-fuzz-undo-total-dirty-
    /// close-discard.md`) — the latter leaves no document for either
    /// checker to say anything meaningful about.
    seed_doc: DocumentId,
}

/// Accumulates the frozen state once a violation fires, so the driving loop
/// can stop at the first one (first-wins, WP3.S7 rule 5).
struct Outcome {
    violation: Option<Violation>,
    final_snapshot: Option<Snapshot>,
    final_ctx: Option<StepCtx>,
}

/// Runs `actions` against a fresh session seeded with `content` at `path`
/// (plan WP7.S2 — a session now opens an arbitrary path, so `DocumentKind`
/// producer selection, including a code or plain document, is reachable
/// from this driver, not just markdown). Deterministic: same input, same
/// result, always — zero wall-clock reads, zero threads, zero subprocesses
/// (WP3.S7 rule 7).
pub fn run(path: &str, content: &str, actions: &[Action]) -> RunResult {
    let path = PathBuf::from(path);
    let mem = Arc::new(Mem::new());
    let _ = mem.save_atomic(&path, content.as_bytes());
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(&mem) as Arc<dyn Vfs + Send + Sync>;

    let mut app = App::new(Buffer::new(content), Some(path.clone()), vfs, None);
    // WP14.S2 (CODE-REVIEW.md rune-fuzz finding 17): `App::new`'s default
    // `pointer_clock` is the real wall clock (`SystemClock`) — harmless
    // today only because this driver never delivers `Msg::Mouse`, so
    // `PointerState`'s multi-click window never actually reads it. Swapped
    // for `ManualClock` (already `pub`, built for exactly this — mirrors
    // `keystate::FixedSpaceProbe`) BEFORE any mouse action exists, so a
    // future `Action::Mouse` never has to retrofit determinism onto a
    // driver that spent real wall-clock time all along — replay would
    // silently stop reproducing the moment a click sequence straddled a
    // click-window boundary at real, non-reproducible speed.
    app.pointer_clock = Box::new(rune_tui::pointer::ManualClock::new());
    app.active_doc_mut().focused = true;
    // Seeds through the same geometry chokepoint `Msg::Resize` uses (plan
    // WP3.S9, gotcha 9) rather than a bare `viewport.set_size` — since
    // WP3, `App::relayout` (called from `sync_view` below) overrides the
    // viewport whenever `frame_width != 0`, so a driver that only set the
    // viewport directly would have it silently overwritten on the very
    // first `sync_view` call.
    app.frame_width = 80;
    app.frame_height = 24;
    app.relayout();
    app.sync_view();

    let seed_doc = app.active;
    let mut state = State {
        app,
        mem,
        path,
        pending_save: None,
        saves_delivered_ok: 0,
        steps: 0,
        seed_doc,
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
            Action::StaleConfirmTimeout(generation) => {
                // Deliberately no `pending_quit` precondition (unlike
                // `ConfirmTimeout` above) -- a stale timer firing after
                // `pending_quit` already cleared entirely, or after a
                // DIFFERENT generation re-armed it, is exactly the
                // production race this variant exists to exercise.
                let msg = Msg::ConfirmTimeout {
                    generation: *generation,
                };
                let tag = MsgTag::ConfirmTimeout {
                    generation: *generation,
                };
                if step_and_check(&mut state, &mut prev, msg, tag, None, &mut outcome) {
                    break 'session;
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
            Action::DirLoaded {
                entries,
                cause,
                generation,
            } => {
                let msg = Msg::DirLoaded {
                    root: PathBuf::from(FUZZ_DIR_ROOT),
                    entries: entries.clone(),
                    cause: *cause,
                    generation: *generation,
                };
                if step_and_check(
                    &mut state,
                    &mut prev,
                    msg,
                    MsgTag::DirLoaded,
                    None,
                    &mut outcome,
                ) {
                    break 'session;
                }
            }
            Action::Highlight { version, spans } => {
                // Resolved against the LIVE buffer version at delivery
                // time (`HighlightVersion`'s own docs) — never a fixed
                // constant, mirroring `Action::ConfirmTimeout`'s rule.
                let live = state.app.active_doc().buffer.version();
                let delivered_version = match version {
                    HighlightVersion::Live => live,
                    HighlightVersion::Stale => live.saturating_sub(1),
                    HighlightVersion::Future => live.saturating_add(1),
                };
                let doc = state.app.active;
                let msg = Msg::Highlighted {
                    doc,
                    version: delivered_version,
                    // Hostile span injection, never a `ParsedTree` — the
                    // fuzzer has no way to synthesize one, and shouldn't:
                    // dispatch still stores a `Spans` payload for any
                    // document (D6), so this keeps exercising the same
                    // clamp/stale-drop invariants it always has.
                    result: Some(rune_tui::runtime::HighlightPayload::Spans(
                        rune_tui::runtime::HighlightResult {
                            spans: crate::action::highlight_spans_from_raw(spans),
                            truncated: false,
                        },
                    )),
                };
                let tag = MsgTag::Highlighted {
                    delivered_version,
                    span_count: spans.len(),
                };
                if step_and_check(&mut state, &mut prev, msg, tag, None, &mut outcome) {
                    break 'session;
                }
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

    // WP6.S4 end-of-session checks — `checks::drive_end_of_session_checks`'s
    // own doc comment carries the full rationale (undo-then-redo drive,
    // skip conditions, G15).
    checks::drive_end_of_session_checks(&mut state, &mut prev, &mut outcome, content);

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
/// produced together with its tag and the bytes it was constructed with —
/// looked up in the per-document snapshot by the ack's OWN `id`, never by
/// whichever document is active at delivery time (see `MsgTag::SaveDone`'s
/// docs; `TODO-fuzz-save-verbatim-help-doc-stale-ack.md`). The snapshot is
/// guaranteed to have an entry for `id`: it was built from every open
/// document at the moment THIS save `Cmd` was constructed, and `id` names
/// exactly the document `trigger_save` built the `Cmd` for — which must
/// have existed then (`trigger_save` bails out before constructing any
/// `Cmd` if its target document doesn't). `save_cmd` (`app.rs`) only ever
/// constructs a `Msg::SaveDone` reply and never returns `None` — the `None`
/// arms below are defensive against `Cmd`'s general contract, not reachable
/// from any real save `Cmd` this driver stores. Never synthesizes
/// `Msg::SaveDone` itself (G14) — it only ever forwards what `cmd.run()`
/// actually returned.
fn discharge_pending_save(state: &mut State) -> Option<(Msg, MsgTag, Vec<u8>)> {
    let (cmd, per_doc_bytes) = state.pending_save.take()?;
    let msg = cmd.run()?;
    let Msg::SaveDone {
        id,
        version,
        result,
        ..
    } = &msg
    else {
        return None;
    };
    let tag = MsgTag::SaveDone {
        id: *id,
        version: *version,
        ok: result.is_ok(),
    };
    let bytes = per_doc_bytes.get(id).cloned().unwrap_or_default();
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
    // fork `/usr/bin/pbpaste`. `state.pending_save` is an `Option`, never a
    // queue, PRECISELY because G9 claims at most one save `Cmd` is ever
    // outstanding — silently overwriting a still-pending one here would
    // drop the first save's `Cmd` on the floor (never delivered, `SAVE-
    // CLEAN-MATCHES-DISK` never even sampling it) and make this driver
    // blind to the exact G9 violation it exists to catch (CODE-REVIEW.md
    // rune-fuzz finding 3). A second in-flight save `Cmd` is therefore a
    // violation in its own right, not a silent overwrite.
    for cmd in effects.cmds {
        if cmd.kind() == CmdKind::Save {
            if state.pending_save.is_some() {
                outcome.violation = Some(Violation {
                    id: "SAVE-SINGLE-FLIGHT",
                    message: "a second save Cmd arrived while one was already pending \
                              (G9: at most one save Cmd may ever be outstanding)"
                        .to_string(),
                });
                outcome.final_snapshot = Some(prev.clone());
                outcome.final_ctx = None;
                return true;
            }
            // Snapshot EVERY open document's content now, at the instant
            // the `Cmd` is constructed — never just `prev.content` (the
            // ACTIVE document's `Snapshot`): `trigger_save` can be called
            // with an id other than `app.active` (a Guard modal's `s`
            // hotkey saves its own prompt's document), so the only
            // reliable way to recover "what bytes was this Cmd actually
            // built with" is to have all candidates on hand and pick the
            // right one once the ack names its `id`.
            let per_doc_bytes = state
                .app
                .documents
                .iter()
                .map(|(&id, doc)| (id, doc.buffer.content().as_bytes().to_vec()))
                .collect();
            state.pending_save = Some((cmd, per_doc_bytes));
        }
    }

    if is_save_done_ok {
        state.saves_delivered_ok += 1;
    }

    let sampled = checks::should_sample(step_index);
    let next = Snapshot::capture(&mut state.app, sampled);
    let disk = state.mem.read(&state.path).ok();
    // `SAVE-CLEAN-MATCHES-DISK` only ever tests this field for `.is_some()`
    // (a save is still outstanding), never its content — any one entry
    // from the per-document snapshot signals that correctly.
    let pending_save_bytes = state
        .pending_save
        .as_ref()
        .and_then(|(_, per_doc)| per_doc.values().next().cloned());

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
        violation = checks::sync_idempotent_check(&mut state.app)
            .or_else(|| checks::wrap_rt_check(&state.app, next.line_count));
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
/// payload.
fn downcast_panic(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "<unknown panic value>".to_string()
    }
}

// `should_sample`, `sync_idempotent_check`, `wrap_rt_check` and the
// end-of-session undo/redo drive (`drive_end_of_session_checks`, which
// subsumes the former `restore_editor_focus`) moved to `checks.rs` (§1.6
// budget) — `step_and_check`/`run` above reach them through `checks::`.
