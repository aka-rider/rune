mod actions;

use std::path::PathBuf;
use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_tui::app::App;
use rune_tui::db::DbBridge;
use rune_tui::document::DocumentId;
use rune_tui::keymap::KeyInput;
use rune_tui::runtime::Cmd;
use rune_tui::workspace;
use rune_vfs::{Mem, Vfs};

use crate::action::Action;
use crate::guard;
use crate::invariant::Violation;
use crate::snapshot::Snapshot;
use crate::step::StepCtx;

use super::RunResult;
use super::checks;
use super::step_exec::{
    discharge_pending_rename, discharge_pending_save, discharge_pending_trash, step_and_check,
};
use super::store_ops::{drain_all_db_ops, drain_all_pending_setup, open_store};

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
pub(super) struct State {
    pub(super) app: App,
    pub(super) mem: Arc<Mem>,
    pub(super) path: PathBuf,
    pub(super) pending_save: Option<(Cmd, std::collections::BTreeMap<DocumentId, Vec<u8>>)>,
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
    pub(super) pending_rename: Option<Cmd>,
    /// The one trash `Cmd` (`CmdKind::Trash`) that can be outstanding at a
    /// time (plan WP3.S3) — same single-slot reasoning as `pending_rename`.
    /// Left undischarged, `Mem::trash` and `Msg::TrashDone` are never
    /// reached from this driver.
    pub(super) pending_trash: Option<Cmd>,
    pub(super) saves_delivered_ok: usize,
    pub(super) steps: usize,
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
    pub(super) seed_doc: DocumentId,
    /// The one untitled draft `App::new` mints before the seeded document
    /// is ever opened — kept as the switch-away target
    /// `Action::DivergeDisk`'s away-and-back reprobe needs, since a probe is
    /// only re-issued by an actual transition through `workspace::
    /// switch_to`, never by re-selecting the document already active.
    pub(super) draft_doc: DocumentId,
    /// Routes every recovery-store op's async reply back into this session
    /// — kept in `Bootstrap` mode for the session's whole
    /// lifetime, never `attach`ed, so every `DbEvent` stays buffered here
    /// for `drain_one_db_op` to pull from deterministically instead of
    /// racing a live `Sender<Msg>`. Present even when `Store::open_in_
    /// memory` itself failed and `app.db` is `None`: harmless, since no op
    /// is ever enqueued onto `app.db_ops` without a live store to enqueue
    /// it through, so every drain attempt just finds nothing pending.
    pub(super) bridge: Arc<DbBridge>,
    /// The stateful `MERGE-NO-INSTANT-REDIVERGENCE` tracker — fed every
    /// checked step by `step_and_check`, told about every `Action::
    /// DivergeDisk` by `diverge_disk` (which runs outside the step cycle
    /// and would otherwise be invisible to it).
    pub(super) rediverge: crate::invariant::RedivergenceTracker,
    /// The stateful `SAVE-AGREES-WITH-DIVERGENCE` tracker — fed every
    /// checked step by `step_and_check`, correlating the step that armed a
    /// save with the step its ack resolves on.
    pub(super) divergent_save: crate::invariant::DivergentSaveTracker,
    /// Bumped on every `Action::DivergeDisk` so repeated occurrences in one
    /// session publish genuinely different bytes each time — a store-backed
    /// session must never externally "publish" the same bytes twice in a
    /// row, since that would classify as `Clean`, not `DiskAhead`/
    /// `Diverged`, defeating the very divergence this action exists to
    /// seed.
    pub(super) diverge_step: u64,
    pub(super) manual_clock: Arc<rune_tui::pointer::ManualClock>,
}

/// Accumulates the frozen state once a violation fires, so the driving loop
/// can stop at the first one (first-wins, WP3.S7 rule 5).
pub(super) struct Outcome {
    pub(super) violation: Option<Violation>,
    pub(super) final_snapshot: Option<Snapshot>,
    pub(super) final_ctx: Option<StepCtx>,
    /// Latched `true` the first time any step's `Snapshot::merge_active`
    /// comes back `true` — `step_and_check` sets this, never cleared once
    /// set, since `RunResult::merge_activated` reports whether the session
    /// EVER reached `Active`, not just whether it ended there.
    pub(super) merge_activated: bool,
}

pub struct Session {
    state: State,
    content: String,
    outcome: Outcome,
    phase: Phase,
}

enum Phase {
    Live(Box<Snapshot>),
    SetupPanicked,
}

fn new_state(
    app: App,
    mem: Arc<Mem>,
    path: PathBuf,
    seed_doc: DocumentId,
    draft_doc: DocumentId,
    bridge: Arc<DbBridge>,
    manual_clock: Arc<rune_tui::pointer::ManualClock>,
) -> State {
    State {
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
        divergent_save: crate::invariant::DivergentSaveTracker::default(),
        diverge_step: 0,
        manual_clock,
    }
}

impl Session {
    pub fn open(path: &str, content: &str) -> Session {
        let path = PathBuf::from(path);
        let mem = Arc::new(Mem::new());
        let _ = mem.save_atomic(&path, content.as_bytes());
        let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(&mem) as Arc<dyn Vfs + Send + Sync>;

        let (bridge, db) = open_store(&vfs);

        let mut app = App::new(Buffer::new(""), None, Arc::clone(&vfs), db);
        // WP14.S2 (CODE-REVIEW.md rune-fuzz finding 17): `App::new`'s default
        // `clock` is the real wall clock (`SystemClock`) — harmless today only
        // because this driver never delivers `Msg::Mouse`, so `PointerState`'s
        // multi-click window never actually reads it. Swapped for `ManualClock`
        // (already `pub`, built for exactly this) BEFORE any mouse action
        // exists, so a future `Action::Mouse` never has to retrofit determinism
        // onto a driver that spent real wall-clock time all along — replay
        // would silently stop reproducing the moment a click sequence
        // straddled a click-window boundary at real, non-reproducible speed.
        let manual_clock = Arc::new(rune_tui::pointer::ManualClock::new());
        app.clock = Arc::clone(&manual_clock) as Arc<dyn rune_tui::pointer::Clock + Send + Sync>;
        let draft_doc = app.active;

        // The session opens its seeded document the same way a real launch or
        // Explorer selection would — through `workspace::open_path`, so a
        // wired store hydrates it (`db_enqueue::load_document`) exactly as
        // production does, rather than the driver hand-assembling a `Document`
        // that was never routed through the store at all. Falls back to the
        // untitled draft only if the open itself refused (never observed with
        // this driver's own `Mem`-backed content, but `open_path` is fallible
        // in general). Setup already reaches the display pipeline, so it runs
        // under the same panic guard every later step gets.
        let setup = guard::catching_panic(|| {
            let seed_doc = workspace::open_path(&mut app, &path).unwrap_or(draft_doc);
            drain_all_pending_setup(&mut app, &bridge);

            app.active = seed_doc;
            app.active_doc_mut().focused = true;
            // Seeds through the same geometry chokepoint `Msg::Resize` uses
            // (plan WP3.S9, gotcha 9) rather than a bare `viewport.set_size` —
            // since WP3, `App::relayout` (called from `sync_view` below)
            // overrides the viewport whenever `frame_width != 0`, so a driver
            // that only set the viewport directly would have it silently
            // overwritten on the very first `sync_view` call.
            app.frame_width = 80;
            app.frame_height = 24;
            app.relayout();
            app.sync_view();
            seed_doc
        });
        match setup {
            Ok(seed_doc) => {
                let mut state =
                    new_state(app, mem, path, seed_doc, draft_doc, bridge, manual_clock);
                let prev = Snapshot::capture(&mut state.app, false);
                let outcome = Outcome {
                    violation: None,
                    final_snapshot: None,
                    final_ctx: None,
                    merge_activated: prev.merge_active,
                };
                Session {
                    state,
                    content: content.to_string(),
                    outcome,
                    phase: Phase::Live(Box::new(prev)),
                }
            }
            Err(violation) => {
                if let Some(db) = app.db.take() {
                    db.shutdown();
                }
                Session {
                    state: new_state(app, mem, path, draft_doc, draft_doc, bridge, manual_clock),
                    content: content.to_string(),
                    outcome: Outcome {
                        violation: Some(violation),
                        final_snapshot: None,
                        final_ctx: None,
                        merge_activated: false,
                    },
                    phase: Phase::SetupPanicked,
                }
            }
        }
    }

    pub fn act(&mut self, action: Action) -> Option<&Violation> {
        if let Phase::Live(prev) = &mut self.phase
            && self.outcome.violation.is_none()
            && !self.state.app.should_quit
        {
            actions::apply(&mut self.state, prev, &mut self.outcome, &action);
        }
        self.outcome.violation.as_ref()
    }

    pub fn key(&mut self, key: KeyInput) -> Option<&Violation> {
        self.act(Action::Key(key))
    }

    pub fn type_(&mut self, text: &str) -> Option<&Violation> {
        self.act(Action::Type(text.to_string()))
    }

    pub fn paste(&mut self, text: &str) -> Option<&Violation> {
        self.act(Action::Paste(text.to_string()))
    }

    pub fn resize(&mut self, width: u16, height: u16) -> Option<&Violation> {
        self.act(Action::Resize(width, height))
    }

    pub fn deliver(&mut self) -> Option<&Violation> {
        self.act(Action::Deliver)
    }

    pub fn deliver_db(&mut self) -> Option<&Violation> {
        self.act(Action::DeliverDb)
    }

    pub fn deliver_db_all(&mut self) -> Option<&Violation> {
        self.act(Action::DeliverDbAll)
    }

    pub fn snapshot(&mut self) -> Snapshot {
        Snapshot::capture(&mut self.state.app, true)
    }

    pub fn app(&self) -> &App {
        &self.state.app
    }

    pub fn app_mut(&mut self) -> &mut App {
        &mut self.state.app
    }

    pub fn finish(mut self) -> RunResult {
        match std::mem::replace(&mut self.phase, Phase::SetupPanicked) {
            Phase::Live(mut prev) => {
                let state = &mut self.state;
                let outcome = &mut self.outcome;
                let prev = &mut prev;

                // Rule 6: discharge any still-pending save before finishing, unless a
                // violation already stopped the session or a quit tore the model down.
                if outcome.violation.is_none()
                    && !state.app.should_quit
                    && let Some((msg, tag, bytes)) = discharge_pending_save(state)
                {
                    step_and_check(state, prev, msg, tag, bytes, outcome);
                }

                // Rule 6b (plan WP5): discharge any still-pending rename before
                // finishing too — a session whose last action left `RenameState::
                // Committing` stuck (no `Action::Deliver` ever followed) must not
                // reach `drive_end_of_session_checks` with the title still able to
                // veto every blur. Same skip conditions as Rule 6.
                if outcome.violation.is_none()
                    && !state.app.should_quit
                    && let Some((msg, tag)) = discharge_pending_rename(state)
                {
                    step_and_check(state, prev, msg, tag, None, outcome);
                }

                // Rule 6c (plan WP3.S3): discharge any still-pending trash before
                // finishing too, same skip conditions as Rules 6/6b.
                if outcome.violation.is_none()
                    && !state.app.should_quit
                    && let Some((msg, tag)) = discharge_pending_trash(state)
                {
                    step_and_check(state, prev, msg, tag, None, outcome);
                }

                // Rule 6d: drain every recovery-store op still pending
                // before the end-of-session undo/redo drive — left undrained, a merge/
                // probe/append-edit ack sitting in `app.db_ops` would just carry over
                // into a `Store` about to be shut down anyway, so this settles the
                // backlog THIS session raised rather than handing it to the next one.
                // Same skip conditions as Rule 6/6b/6c.
                if outcome.violation.is_none() && !state.app.should_quit {
                    drain_all_db_ops(state, prev, outcome);
                }

                // WP6.S4 end-of-session checks — `checks::drive_end_of_session_checks`'s
                // own doc comment carries the full rationale (undo-then-redo drive,
                // skip conditions, G15).
                checks::drive_end_of_session_checks(state, prev, outcome, &self.content);

                // Deterministically joins the store's writer/reader threads (mirrors
                // `rune-cli::main`'s own exit-path shutdown, `Db::shutdown`'s own doc
                // comment) — a per-session `Store` minted for every one of thousands
                // of fuzz cases must not leak its OS threads onto the next one.
                if let Some(db) = state.app.db.take() {
                    db.shutdown();
                }

                RunResult {
                    violation: outcome.violation.take(),
                    steps: state.steps,
                    final_content: std::mem::take(&mut prev.content),
                    final_snapshot: outcome.final_snapshot.take(),
                    final_ctx: outcome.final_ctx.take(),
                    merge_activated: outcome.merge_activated,
                }
            }
            Phase::SetupPanicked => RunResult {
                violation: self.outcome.violation.take(),
                steps: self.state.steps,
                final_content: std::mem::take(&mut self.content),
                final_snapshot: None,
                final_ctx: None,
                merge_activated: false,
            },
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Some(db) = self.state.app.db.take() {
            db.shutdown();
        }
    }
}
