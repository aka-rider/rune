mod actions;

use std::path::PathBuf;
use std::sync::Arc;

use ratatui::layout::Rect;

use rune_core::buffer::Buffer;
use rune_tui::app::App;
use rune_tui::db::DbBridge;
use rune_tui::document::DocumentId;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::pointer::{MouseButton, MouseInput, MouseKind};
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
use super::discharge::{
    discharge_pending_highlight, discharge_pending_rename, discharge_pending_save,
    discharge_pending_trash,
};
use super::step_exec::step_and_check;
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
    pub(super) pending_highlights: std::collections::VecDeque<Cmd>,
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
    pub(super) disk_diverged_since_publish: bool,
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

/// `GlobalCommand::FocusTabs`'s `^t` chord (`global::GLOBAL_BINDINGS`).
const FOCUS_TABS: KeyInput = KeyInput {
    code: KeyCode::Char('t'),
    mods: Mods {
        shift: false,
        alt: false,
        ctrl: true,
        sup: false,
    },
};

const UP: KeyInput = KeyInput {
    code: KeyCode::Up,
    mods: Mods::NONE,
};

const DOWN: KeyInput = KeyInput {
    code: KeyCode::Down,
    mods: Mods::NONE,
};

const ENTER: KeyInput = KeyInput {
    code: KeyCode::Enter,
    mods: Mods::NONE,
};

/// The Tabs pane's own paint rect, the same `layout::geometry` call the
/// renderer and `opentabs::mouse` both make.
fn tabs_inner_rect(app: &App) -> Rect {
    let area = Rect::new(0, 0, app.frame_width, app.frame_height);
    rune_tui::layout::geometry(area, app).tabs_inner
}

/// The seeded-session inputs `Session::boot` threads through to whichever
/// of `live_session`/`panicked_session` handles `open_seed_document`'s
/// outcome — bundled so passing them along stays under clippy's argument
/// limit.
struct Seed {
    mem: Arc<Mem>,
    path: PathBuf,
    content: String,
    bridge: Arc<DbBridge>,
    manual_clock: Arc<rune_tui::pointer::ManualClock>,
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
        pending_highlights: std::collections::VecDeque::new(),
        saves_delivered_ok: 0,
        steps: 0,
        seed_doc,
        draft_doc,
        bridge,
        rediverge: crate::invariant::RedivergenceTracker::default(),
        divergent_save: crate::invariant::DivergentSaveTracker::default(),
        diverge_step: 0,
        disk_diverged_since_publish: false,
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
        Session::boot(mem, path, content.to_string(), bridge, db)
    }

    /// The store-injecting variant of [`Session::open`]: the caller owns the
    /// `Mem` (so it can outlive this session — a "restart" reopens a second
    /// session over the same disk) and the `Db` (so the `Store` can live at
    /// a real on-disk path, carry a liveness override, or start degraded —
    /// none of which `open`'s own in-memory store can express). The seeded
    /// document's content is whatever `mem` already holds at `path`; this
    /// constructor writes nothing to disk.
    pub fn open_with_db(path: &str, mem: Arc<Mem>, db: rune_tui::db::Db) -> Session {
        let path = PathBuf::from(path);
        let content = mem
            .read(&path)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .unwrap_or_default();
        let bridge = Arc::clone(&db.bridge);
        Session::boot(mem, path, content, bridge, Some(db))
    }

    /// Builds the `App` this session drives, with the real wall clock
    /// (`App::new`'s default) swapped for a `ManualClock` before any
    /// mouse action exists (WP14.S2, CODE-REVIEW.md rune-fuzz finding 17)
    /// — a driver that only swapped it in later would have already spent
    /// real wall-clock time, and replay would silently stop reproducing
    /// the moment a click sequence straddled a click-window boundary at
    /// real, non-reproducible speed.
    fn new_app(
        vfs: Arc<dyn Vfs + Send + Sync>,
        db: Option<rune_tui::db::Db>,
    ) -> (App, Arc<rune_tui::pointer::ManualClock>) {
        let mut app = App::new(Buffer::new(""), None, vfs, db);
        let manual_clock = Arc::new(rune_tui::pointer::ManualClock::new());
        app.clock = Arc::clone(&manual_clock) as Arc<dyn rune_tui::pointer::Clock + Send + Sync>;
        (app, manual_clock)
    }

    /// Opens the session's seeded document the same way a real launch or
    /// Explorer selection would — through `workspace::open_path`, so a
    /// wired store hydrates it (`db_enqueue::load_document`) exactly as
    /// production does, rather than the driver hand-assembling a
    /// `Document` that was never routed through the store at all. Falls
    /// back to the untitled draft only if the open itself refused (never
    /// observed with this driver's own `Mem`-backed content, but
    /// `open_path` is fallible in general).
    fn open_seed_document(
        app: &mut App,
        path: &std::path::Path,
        draft_doc: DocumentId,
        bridge: &DbBridge,
    ) -> DocumentId {
        let seed_doc = workspace::open_path(app, path).unwrap_or(draft_doc);
        drain_all_pending_setup(app, bridge);

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
    }

    fn live_session(app: App, seed: Seed, seed_doc: DocumentId, draft_doc: DocumentId) -> Session {
        let mut state = new_state(
            app,
            seed.mem,
            seed.path,
            seed_doc,
            draft_doc,
            seed.bridge,
            seed.manual_clock,
        );
        let prev = Snapshot::capture(&mut state.app, false);
        let outcome = Outcome {
            violation: None,
            final_snapshot: None,
            final_ctx: None,
            merge_activated: prev.merge_active,
        };
        Session {
            state,
            content: seed.content,
            outcome,
            phase: Phase::Live(Box::new(prev)),
        }
    }

    fn panicked_session(
        mut app: App,
        seed: Seed,
        draft_doc: DocumentId,
        violation: Violation,
    ) -> Session {
        if let Some(db) = app.db.take() {
            db.shutdown();
        }
        Session {
            state: new_state(
                app,
                seed.mem,
                seed.path,
                draft_doc,
                draft_doc,
                seed.bridge,
                seed.manual_clock,
            ),
            content: seed.content,
            outcome: Outcome {
                violation: Some(violation),
                final_snapshot: None,
                final_ctx: None,
                merge_activated: false,
            },
            phase: Phase::SetupPanicked,
        }
    }

    fn boot(
        mem: Arc<Mem>,
        path: PathBuf,
        content: String,
        bridge: Arc<DbBridge>,
        db: Option<rune_tui::db::Db>,
    ) -> Session {
        let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(&mem) as Arc<dyn Vfs + Send + Sync>;
        let (mut app, manual_clock) = Session::new_app(vfs, db);
        let draft_doc = app.active;

        // Setup already reaches the display pipeline, so it runs under the
        // same panic guard every later step gets.
        let setup = guard::catching_panic(|| {
            Session::open_seed_document(&mut app, &path, draft_doc, &bridge)
        });
        let seed = Seed {
            mem,
            path,
            content,
            bridge,
            manual_clock,
        };
        match setup {
            Ok(seed_doc) => Session::live_session(app, seed, seed_doc, draft_doc),
            Err(violation) => Session::panicked_session(app, seed, draft_doc, violation),
        }
    }

    pub fn act(&mut self, action: Action) -> Option<&Violation> {
        if let Phase::Live(prev) = &mut self.phase
            && self.outcome.violation.is_none()
            && !self.state.app.should_quit
        {
            actions::apply(&mut self.state, prev, &mut self.outcome, action);
        }
        self.outcome.violation.as_ref()
    }

    pub fn key(&mut self, key: KeyInput) -> Option<&Violation> {
        self.act(Action::Key(key))
    }

    pub fn mouse(&mut self, input: rune_tui::pointer::MouseInput) -> Option<&Violation> {
        self.act(Action::Mouse(input))
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

    /// Renders the session's `App` into a `width`x`height` `TestBackend`
    /// and returns every row as its own `String` — delegates to
    /// `rune_tui::testgrid::grid`, the crate's sole `TestBackend` site.
    pub fn grid(&mut self, width: u16, height: u16) -> Vec<String> {
        rune_tui::testgrid::grid(&self.state.app, width, height)
    }

    /// Renders the session's `App` and returns row `y` only — delegates to
    /// `rune_tui::testgrid::row`.
    pub fn row(&mut self, y: u16, width: u16, height: u16) -> String {
        rune_tui::testgrid::row(&self.state.app, y, width, height)
    }

    /// Switches to the tab at `index` (in `app.documents.order()`) the way a
    /// user actually would: `^t` focuses the Tabs pane, `Up` repeated
    /// clamps the cursor to the top, `Down` walks it to `index`, and
    /// `Enter` opens it — the real chord set `opentabs::TABS_BINDINGS`
    /// resolves, never `workspace::switch_to` directly.
    pub fn switch_tab_by_index(&mut self, index: usize) -> Option<&Violation> {
        self.focus_and_select_tab(index);
        self.key(ENTER)
    }

    /// Switches to the tab at `index` the way a mouse click would: focuses
    /// the Tabs pane and scrolls `index` into view via the same real key
    /// sequence `switch_tab_by_index` uses, then synthesizes the
    /// double-click (`Down`+`Up` twice, `PointerState::register_row_click`'s
    /// own activation rule) on that row's cells, computed from the same
    /// `layout::geometry` rect the renderer paints the Tabs pane into.
    pub fn switch_tab_by_click(&mut self, index: usize) -> Option<&Violation> {
        self.focus_and_select_tab(index);

        let rect = tabs_inner_rect(&self.state.app);
        let order_len = self.state.app.documents.order().len();
        let window = self
            .state
            .app
            .tabs
            .nav
            .window(order_len, rect.height as usize);
        let local_row = index.saturating_sub(window.start) as u16;
        let column = rect.x;
        let row = rect.y + local_row;

        for _ in 0..2 {
            self.mouse(MouseInput {
                kind: MouseKind::Down(MouseButton::Left),
                column,
                row,
                shift: false,
                alt: false,
                ctrl: false,
            });
            self.mouse(MouseInput {
                kind: MouseKind::Up(MouseButton::Left),
                column,
                row,
                shift: false,
                alt: false,
                ctrl: false,
            });
        }
        self.outcome.violation.as_ref()
    }

    /// Focuses the Tabs pane and walks its cursor to `index` via `Up`/`Down`
    /// — the shared prefix `switch_tab_by_index` and `switch_tab_by_click`
    /// both need before they diverge on how the tab is actually opened.
    fn focus_and_select_tab(&mut self, index: usize) {
        self.key(FOCUS_TABS);
        let order_len = self.state.app.documents.order().len();
        for _ in 0..order_len {
            self.key(UP);
        }
        for _ in 0..index {
            self.key(DOWN);
        }
    }

    pub fn app(&self) -> &App {
        &self.state.app
    }

    pub fn app_mut(&mut self) -> &mut App {
        &mut self.state.app
    }

    pub fn saves_delivered_ok(&self) -> usize {
        self.state.saves_delivered_ok
    }

    pub fn disk_diverged_since_publish(&self) -> bool {
        self.state.disk_diverged_since_publish
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

                while outcome.violation.is_none() && !state.app.should_quit {
                    let Some((msg, tag)) = discharge_pending_highlight(state) else {
                        break;
                    };
                    step_and_check(state, prev, msg, tag, None, outcome);
                }

                // Rule 6e: drain every recovery-store op still pending
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
