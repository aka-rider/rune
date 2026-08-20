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
use rune_vfs::{Mem, Vfs, VfsTestExt};

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

pub(super) struct State {
    pub(super) app: App,
    pub(super) mem: Arc<Mem>,
    pub(super) path: PathBuf,
    pub(super) pending_save: Option<(Cmd, std::collections::BTreeMap<DocumentId, Vec<u8>>)>,
    pub(super) pending_rename: Option<Cmd>,
    pub(super) pending_trash: Option<Cmd>,
    pub(super) pending_highlights: std::collections::VecDeque<Cmd>,
    pub(super) saves_delivered_ok: usize,
    pub(super) steps: usize,
    pub(super) seed_doc: DocumentId,
    pub(super) draft_doc: DocumentId,
    pub(super) bridge: Arc<DbBridge>,
    pub(super) rediverge: crate::invariant::RedivergenceTracker,
    pub(super) divergent_save: crate::invariant::DivergentSaveTracker,
    pub(super) diverge_step: u64,
    pub(super) disk_diverged_since_publish: bool,
    pub(super) manual_clock: Arc<rune_tui::pointer::ManualClock>,
}

pub(super) struct Outcome {
    pub(super) violation: Option<Violation>,
    pub(super) final_snapshot: Option<Snapshot>,
    pub(super) final_ctx: Option<StepCtx>,
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

fn tabs_inner_rect(app: &App) -> Rect {
    let area = Rect::new(0, 0, app.frame_width, app.frame_height);
    rune_tui::layout::geometry(area, app).tabs_inner
}

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

    fn new_app(
        vfs: Arc<dyn Vfs + Send + Sync>,
        db: Option<rune_tui::db::Db>,
    ) -> (App, Arc<rune_tui::pointer::ManualClock>) {
        let mut app = App::new(Buffer::new(""), None, vfs, db);
        let manual_clock = Arc::new(rune_tui::pointer::ManualClock::new());
        app.clock = Arc::clone(&manual_clock) as Arc<dyn rune_tui::pointer::Clock + Send + Sync>;
        (app, manual_clock)
    }

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

    pub fn grid(&mut self, width: u16, height: u16) -> Vec<String> {
        rune_tui::testgrid::grid(&self.state.app, width, height)
    }

    pub fn row(&mut self, y: u16, width: u16, height: u16) -> String {
        rune_tui::testgrid::row(&self.state.app, y, width, height)
    }

    pub fn switch_tab_by_index(&mut self, index: usize) -> Option<&Violation> {
        self.focus_and_select_tab(index);
        self.key(ENTER)
    }

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

                if outcome.violation.is_none()
                    && !state.app.should_quit
                    && let Some((msg, tag, bytes)) = discharge_pending_save(state)
                {
                    step_and_check(state, prev, msg, tag, bytes, outcome);
                }

                if outcome.violation.is_none()
                    && !state.app.should_quit
                    && let Some((msg, tag)) = discharge_pending_rename(state)
                {
                    step_and_check(state, prev, msg, tag, None, outcome);
                }

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

                if outcome.violation.is_none() && !state.app.should_quit {
                    drain_all_db_ops(state, prev, outcome);
                }

                checks::drive_end_of_session_checks(state, prev, outcome, &self.content);

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
