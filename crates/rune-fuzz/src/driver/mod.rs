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
mod store_ops;

use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_tui::app::App;
use rune_tui::db::DbBridge;
use rune_tui::document::DocumentId;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::runtime::{Cmd, Msg, PasteTarget};
use rune_tui::workspace;
use rune_vfs::{Mem, Vfs};

use step_exec::{
    discharge_pending_rename, discharge_pending_save, discharge_pending_trash, drain_one_db_op,
    highlight_step, highlight_tree_step, key_step, step_and_check,
};
pub use store_ops::wait_for_db_op;
use store_ops::{diverge_disk, drain_all_db_ops, drain_all_pending_setup, open_store};

/// The fixed root every `Action::DirLoaded` targets (plan WP4.S6) — only
/// `entries`/`cause` vary; the root itself isn't the thing under fuzz here.
const FUZZ_DIR_ROOT: &str = "/fuzz/dir";

use crate::action::Action;
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
    /// True iff `Snapshot::merge_active` was ever true on any step of this
    /// session (non-vacuous merge coverage) — unlike
    /// `final_snapshot`, tracked on EVERY step, not just a violating one,
    /// since a session's own resolver work can legitimately exit `Active`
    /// again (a full resolution, an auto-exit on tab switch) before the
    /// session ends.
    pub merge_activated: bool,
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
    /// The one untitled draft `App::new` mints before the seeded document
    /// is ever opened — kept as the switch-away target
    /// `Action::DivergeDisk`'s away-and-back reprobe needs, since a probe is
    /// only re-issued by an actual transition through `workspace::
    /// switch_to`, never by re-selecting the document already active.
    draft_doc: DocumentId,
    /// Routes every recovery-store op's async reply back into this session
    /// — kept in `Bootstrap` mode for the session's whole
    /// lifetime, never `attach`ed, so every `DbEvent` stays buffered here
    /// for `drain_one_db_op` to pull from deterministically instead of
    /// racing a live `Sender<Msg>`. Present even when `Store::open_in_
    /// memory` itself failed and `app.db` is `None`: harmless, since no op
    /// is ever enqueued onto `app.db_ops` without a live store to enqueue
    /// it through, so every drain attempt just finds nothing pending.
    bridge: Arc<DbBridge>,
    /// The stateful `MERGE-NO-INSTANT-REDIVERGENCE` tracker — fed every
    /// checked step by `step_and_check`, told about every `Action::
    /// DivergeDisk` by `diverge_disk` (which runs outside the step cycle
    /// and would otherwise be invisible to it).
    rediverge: crate::invariant::RedivergenceTracker,
    /// Bumped on every `Action::DivergeDisk` so repeated occurrences in one
    /// session publish genuinely different bytes each time — a store-backed
    /// session must never externally "publish" the same bytes twice in a
    /// row, since that would classify as `Clean`, not `DiskAhead`/
    /// `Diverged`, defeating the very divergence this action exists to
    /// seed.
    diverge_step: u64,
}

/// Accumulates the frozen state once a violation fires, so the driving loop
/// can stop at the first one (first-wins, WP3.S7 rule 5).
struct Outcome {
    violation: Option<Violation>,
    final_snapshot: Option<Snapshot>,
    final_ctx: Option<StepCtx>,
    /// Latched `true` the first time any step's `Snapshot::merge_active`
    /// comes back `true` — `step_and_check` sets this, never cleared once
    /// set, since `RunResult::merge_activated` reports whether the session
    /// EVER reached `Active`, not just whether it ended there.
    merge_activated: bool,
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

    let (bridge, db) = open_store(&vfs);

    let mut app = App::new(Buffer::new(""), None, Arc::clone(&vfs), db);
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
    let draft_doc = app.active;

    // The session opens its seeded document the same way a real launch or
    // Explorer selection would — through `workspace::open_path`, so a
    // wired store hydrates it (`db_enqueue::load_document`) exactly as
    // production does, rather than the driver hand-assembling a `Document`
    // that was never routed through the store at all. Falls back to the
    // untitled draft only if the open itself refused (never observed with
    // this driver's own `Mem`-backed content, but `open_path` is fallible
    // in general).
    let seed_doc = workspace::open_path(&mut app, &path).unwrap_or(draft_doc);
    drain_all_pending_setup(&mut app, &bridge);

    app.active = seed_doc;
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
        draft_doc,
        bridge,
        rediverge: crate::invariant::RedivergenceTracker::default(),
        diverge_step: 0,
    };
    let mut prev = Snapshot::capture(&mut state.app, false);
    let mut outcome = Outcome {
        violation: None,
        final_snapshot: None,
        final_ctx: None,
        merge_activated: prev.merge_active,
    };

    'session: for action in actions {
        if state.app.should_quit {
            break;
        }
        match action {
            Action::FailNextSave => {
                state.mem.fail_next_save(io::ErrorKind::PermissionDenied);
            }
            Action::DivergeDisk => {
                if diverge_disk(&mut state, &mut prev, &mut outcome) {
                    break 'session;
                }
            }
            Action::DeliverDb => {
                let bridge = Arc::clone(&state.bridge);
                if let Some((msg, tag)) = drain_one_db_op(&mut state, &bridge)
                    && step_and_check(&mut state, &mut prev, msg, tag, None, &mut outcome)
                {
                    break 'session;
                }
            }
            Action::DeliverDbAll => {
                if drain_all_db_ops(&mut state, &mut prev, &mut outcome) {
                    break 'session;
                }
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
                    && step_and_check(&mut state, &mut prev, msg, tag, bytes, &mut outcome)
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
                let (msg, tag) = highlight_step(&state, *version, spans);
                if step_and_check(&mut state, &mut prev, msg, tag, None, &mut outcome) {
                    break 'session;
                }
            }
            Action::HighlightTree {
                version,
                fixture,
                base,
            } => {
                let (msg, tag) = highlight_tree_step(&state, *version, *fixture, *base);
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
        step_and_check(&mut state, &mut prev, msg, tag, bytes, &mut outcome);
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

    // Rule 6d: drain every recovery-store op still pending
    // before the end-of-session undo/redo drive — left undrained, a merge/
    // probe/append-edit ack sitting in `app.db_ops` would just carry over
    // into a `Store` about to be shut down anyway, so this settles the
    // backlog THIS session raised rather than handing it to the next one.
    // Same skip conditions as Rule 6/6b/6c.
    if outcome.violation.is_none() && !state.app.should_quit {
        drain_all_db_ops(&mut state, &mut prev, &mut outcome);
    }

    // WP6.S4 end-of-session checks — `checks::drive_end_of_session_checks`'s
    // own doc comment carries the full rationale (undo-then-redo drive,
    // skip conditions, G15).
    checks::drive_end_of_session_checks(&mut state, &mut prev, &mut outcome, content);

    // Deterministically joins the store's writer/reader threads (mirrors
    // `rune-cli::main`'s own exit-path shutdown, `Db::shutdown`'s own doc
    // comment) — a per-session `Store` minted for every one of thousands
    // of fuzz cases must not leak its OS threads onto the next one.
    if let Some(db) = state.app.db.take() {
        db.shutdown();
    }

    RunResult {
        violation: outcome.violation,
        steps: state.steps,
        final_content: prev.content,
        final_snapshot: outcome.final_snapshot,
        final_ctx: outcome.final_ctx,
        merge_activated: outcome.merge_activated,
    }
}

// `key_step`, `discharge_pending_save`, `step_and_check`,
// `run_update_catching_panic`, and `downcast_panic` moved into the
// `step_exec` submodule (500-line budget) — `run` above reaches
// them through the unqualified imports above. `should_sample`,
// `sync_idempotent_check`, `wrap_rt_check` and the end-of-session
// undo/redo drive (`drive_end_of_session_checks`, which subsumes the
// former `restore_editor_focus`) live in the `checks` submodule —
// `step_exec`/`run` reach those through `checks::`. `drain_all_pending_
// setup`, `drain_all_db_ops`, and `diverge_disk` live in the `store_ops`
// submodule — `run` reaches those the same unqualified way.
