//! The deterministic engine: drives the real `rune_tui::app::update` against
//! an in-memory `Vfs`, with no terminal, no clock, and no subprocess — a
//! drain loop scoped to the seam this crate actually has: `App::new` +
//! `app::update` + `Cmd` (WP2's tagged struct) + `Mem`.
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
mod step_exec;

use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_tui::app::App;
use rune_tui::document::DocumentId;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::runtime::{Cmd, Msg, PasteTarget};
use rune_vfs::{Mem, Vfs};

use step_exec::{
    discharge_pending_rename, discharge_pending_save, discharge_pending_trash, key_step,
    step_and_check,
};

/// The fixed root every `Action::DirLoaded` targets (plan WP4.S6) — only
/// `entries`/`cause` vary; the root itself isn't the thing under fuzz here.
const FUZZ_DIR_ROOT: &str = "/fuzz/dir";

use crate::action::{Action, HighlightVersion};
use crate::invariant::Violation;
use crate::snapshot::Snapshot;
use crate::step::{MsgTag, StepCtx};

/// The default seeded file path (plan WP7.S2): every `SEEDS` entry
/// inherited from before this package pairs with this path, and the script
/// codec's optional `path` line defaults to it when absent — so the
/// checked-in `repros/tripwire-clean.rune` (written before sessions carried
/// a path) still decodes unchanged.
pub const DOC_PATH: &str = "/fuzz/doc.md";

/// The result of driving one whole session. `final_snapshot`/`final_ctx`
/// are frozen at the violating step (`None` on a clean run).
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
    /// The one no-store rename `Cmd` (`CmdKind::Rename`) that can be
    /// outstanding at a time (structurally: `rename::begin` refuses a
    /// second commit while `RenameState::Committing` — `in_flight()` —
    /// holds). An `Option`, not a queue, for the same reason `pending_save`
    /// is: at most one is ever produced before this one resolves. Left
    /// undischarged, `app.rename` never leaves `Committing`, and every
    /// later blur attempt — including the end-of-session drive's own `^E`
    /// — is permanently refused (`rename::begin`'s rename-in-flight guard),
    /// which is exactly the fuzz-driver gap `discharge_pending_rename`
    /// closes (plan WP5).
    pending_rename: Option<Cmd>,
    /// The one trash `Cmd` (`CmdKind::Trash`) that can be outstanding at a
    /// time (plan WP3.S3) — same single-slot reasoning as `pending_rename`.
    /// Left undischarged, `Mem::trash` and `Msg::TrashDone` are never
    /// reached from this driver.
    pending_trash: Option<Cmd>,
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
    // for `ManualClock` (already `pub`, built for exactly this) BEFORE any
    // mouse action exists, so a
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
        pending_rename: None,
        pending_trash: None,
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
                if let Some((msg, tag)) = discharge_pending_rename(&mut state)
                    && step_and_check(&mut state, &mut prev, msg, tag, None, &mut outcome)
                {
                    break 'session;
                }
                if let Some((msg, tag)) = discharge_pending_trash(&mut state)
                    && step_and_check(&mut state, &mut prev, msg, tag, None, &mut outcome)
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
                let msg = Msg::ClipboardRead {
                    text: s.clone(),
                    target,
                };
                if step_and_check(&mut state, &mut prev, msg, tag, None, &mut outcome) {
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
                    // Hostile span injection into one region's SPAN
                    // channel, never a `ParsedTree` — the fuzzer has no way
                    // to synthesize one, and shouldn't. A reply describes
                    // the document's whole region layout, so this one is a
                    // single span-backed region: no coordinate map (its
                    // spans are already buffer offsets, exactly like a
                    // markdown fence's), and nothing for the tree channel.
                    // The render-path query reads it back the same way it
                    // reads any other region, so `HL-CLAMPED`/
                    // `HL-STALE-DROP` keep testing the real clamp.
                    result: Some(rune_tui::highlight::HighlightReply {
                        regions: vec![rune_tui::highlight::RegionResult {
                            map: rune_tui::linemap::LineMap::default(),
                            payload: Some(rune_tui::highlight::RegionPayload::Spans(
                                crate::action::highlight_spans_from_raw(spans),
                            )),
                        }],
                        truncated: false,
                    }),
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

    // Rule 6b (plan WP5): discharge any still-pending rename before
    // finishing too — a session whose last action left `RenameState::
    // Committing` stuck (no `Action::Deliver` ever followed) must not
    // reach `drive_end_of_session_checks` with the title still able to
    // veto every blur. Same skip conditions as Rule 6.
    if outcome.violation.is_none()
        && !state.app.should_quit
        && let Some((msg, tag)) = discharge_pending_rename(&mut state)
    {
        step_and_check(&mut state, &mut prev, msg, tag, None, &mut outcome);
    }

    // Rule 6c (plan WP3.S3): discharge any still-pending trash before
    // finishing too, same skip conditions as Rules 6/6b.
    if outcome.violation.is_none()
        && !state.app.should_quit
        && let Some((msg, tag)) = discharge_pending_trash(&mut state)
    {
        step_and_check(&mut state, &mut prev, msg, tag, None, &mut outcome);
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

// `key_step`, `discharge_pending_save`, `step_and_check`,
// `run_update_catching_panic`, and `downcast_panic` moved into the
// `step_exec` submodule (500-line budget) — `run` above reaches
// them through the unqualified imports above. `should_sample`,
// `sync_idempotent_check`, `wrap_rt_check` and the end-of-session
// undo/redo drive (`drive_end_of_session_checks`, which subsumes the
// former `restore_editor_focus`) live in the `checks` submodule —
// `step_exec`/`run` reach those through `checks::`.
