use std::collections::HashMap;
use std::num::NonZeroU64;
use std::path::PathBuf;
use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_vfs::Vfs;

use crate::db::Db;
use crate::dispatch;
use crate::document::{Document, DocumentId};
use crate::document_map::DocumentMap;
use crate::explorer::Explorer;
use crate::guard::GuardPrompt;
use crate::keymap::QuitKey;
use crate::messages::MessageLog;
use crate::opentabs::OpenTabs;
use crate::pane::Pane;
use crate::runtime::{Effects, Msg};
use crate::save;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameSize {
    pub width: u16,
    pub height: u16,
}

impl FrameSize {
    pub const fn new(width: u16, height: u16) -> FrameSize {
        FrameSize { width, height }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct QuitIntent {
    pub pending: std::collections::BTreeMap<DocumentId, u64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum QuitNegotiation {
    #[default]
    Idle,
    ConfirmArmed(QuitKey, crate::generation::QuitGen),
    SaveFanOut(QuitIntent),
}

impl QuitNegotiation {
    pub fn fan_out(&self) -> Option<&QuitIntent> {
        match self {
            QuitNegotiation::SaveFanOut(intent) => Some(intent),
            QuitNegotiation::Idle | QuitNegotiation::ConfirmArmed(..) => None,
        }
    }

    pub fn fan_out_mut(&mut self) -> Option<&mut QuitIntent> {
        match self {
            QuitNegotiation::SaveFanOut(intent) => Some(intent),
            QuitNegotiation::Idle | QuitNegotiation::ConfirmArmed(..) => None,
        }
    }
}

pub struct App {
    pub documents: DocumentMap,
    pub active: DocumentId,
    next_doc_id: NonZeroU64,
    pub vfs: Arc<dyn Vfs + Send + Sync>,
    pub(crate) focus: Pane,
    pub splits: crate::layout::Splits,
    pub frame: Option<FrameSize>,
    pub explorer: Explorer,
    pub tabs: OpenTabs,
    pub help_doc: Option<DocumentId>,
    pub help_return_to: crate::returnto::ReturnTo,
    pub title: crate::title::TitleField,
    pub rename: crate::rename::RenameState,
    pub(crate) next_rename_gen: crate::generation::GenCounter<crate::generation::Rename>,
    pub merge: crate::merge::MergeState,
    pub(crate) next_merge_gen: crate::generation::GenCounter<crate::generation::Merge>,
    pub diff: Option<crate::diff_view::DiffView>,
    pub db: Option<Db>,
    pub db_ops: HashMap<u64, crate::db::PendingOp>,
    pub file_bindings: HashMap<i64, crate::db::FileBinding>,
    pub db_banner: Option<String>,
    pub pending_save_confirm: Option<(DocumentId, crate::generation::SaveConfirmGen)>,
    pub(crate) next_save_confirm_gen: crate::generation::GenCounter<crate::generation::SaveConfirm>,
    pub pending_close_on_save: Option<DocumentId>,
    pub quit: QuitNegotiation,
    pub(crate) next_quit_gen: crate::generation::GenCounter<crate::generation::Quit>,
    pub pointer: crate::pointer::PointerState,
    pub clock: Arc<dyn crate::pointer::Clock + Send + Sync>,
    pub binding_set: crate::keymap::BindingSet,
    pub guard: Option<GuardPrompt>,
    pub(crate) trash: crate::trash::TrashState,
    pub(crate) next_trash_gen: crate::generation::GenCounter<crate::generation::Trash>,
    pub messages: MessageLog,
    pub(crate) overlay: crate::overlay::Overlay,
    pub(crate) next_filesearch_gen: crate::generation::GenCounter<crate::generation::FileSearch>,
    pub(crate) next_projectsearch_gen:
        crate::generation::GenCounter<crate::generation::ProjectSearch>,
    pub(crate) last_search_query: Option<String>,
    pub(crate) search_history: crate::history_persistence::HistoryPersistence,
    pub(crate) next_search_history_gen:
        crate::generation::GenCounter<crate::generation::SearchHistory>,
    pub(crate) next_palette_gen: crate::generation::GenCounter<crate::generation::Palette>,
    pub(crate) command_history: crate::history_persistence::HistoryPersistence,
    pub should_quit: bool,
    pub theme: crate::theme::Theme,
    pub icon_tier: crate::theme::icons::IconTier,
    pub graphics: crate::graphics::GraphicsCaps,
    pub image_ids: crate::graphics::TerminalImageAllocator,
    pub root: Option<PathBuf>,
    pub(crate) timers: Arc<crate::runtime::TimerService>,
    pub nav_history: crate::navhistory::NavHistory,
    pub keyboard_flags: Option<termina::escape::csi::KittyKeyboardFlags>,
}

impl App {
    pub fn new(
        buffer: Buffer,
        file_path: Option<PathBuf>,
        vfs: Arc<dyn Vfs + Send + Sync>,
        db: Option<Db>,
    ) -> App {
        let mut document = Document::new(buffer);
        if let Some(path) = file_path {
            document.bind_path(path);
        }

        let id = DocumentId(NonZeroU64::MIN);
        let documents = DocumentMap::new(id, document);

        App {
            documents,
            active: id,
            next_doc_id: NonZeroU64::MIN.saturating_add(1),
            vfs,
            focus: Pane::Editor,
            splits: crate::layout::Splits::default(),
            frame: None,
            explorer: Explorer::default(),
            tabs: OpenTabs::new(),
            help_doc: None,
            help_return_to: crate::returnto::ReturnTo::none(),
            title: crate::title::TitleField::default(),
            rename: crate::rename::RenameState::default(),
            next_rename_gen: crate::generation::GenCounter::default(),
            merge: crate::merge::MergeState::default(),
            next_merge_gen: crate::generation::GenCounter::default(),
            diff: None,
            db,
            db_ops: HashMap::new(),
            file_bindings: HashMap::new(),
            db_banner: None,
            pending_save_confirm: None,
            next_save_confirm_gen: crate::generation::GenCounter::default(),
            pending_close_on_save: None,
            quit: QuitNegotiation::default(),
            next_quit_gen: crate::generation::GenCounter::default(),
            pointer: crate::pointer::PointerState::default(),
            clock: Arc::new(crate::pointer::SystemClock),
            binding_set: crate::keymap::BindingSet::default(),
            guard: None,
            trash: crate::trash::TrashState::default(),
            next_trash_gen: crate::generation::GenCounter::default(),
            messages: MessageLog::new(),
            overlay: crate::overlay::Overlay::None,
            next_filesearch_gen: crate::generation::GenCounter::default(),
            next_projectsearch_gen: crate::generation::GenCounter::default(),
            last_search_query: None,
            search_history: crate::history_persistence::HistoryPersistence::new(),
            next_search_history_gen: crate::generation::GenCounter::default(),
            next_palette_gen: crate::generation::GenCounter::default(),
            command_history: crate::history_persistence::HistoryPersistence::new(),
            should_quit: false,
            theme: crate::theme::Theme::catppuccin_mocha(false),
            icon_tier: crate::theme::icons::IconTier::Unicode,
            graphics: crate::graphics::GraphicsCaps::default(),
            image_ids: crate::graphics::TerminalImageAllocator::default(),
            root: None,
            timers: crate::runtime::TimerService::new(),
            nav_history: crate::navhistory::NavHistory::default(),
            keyboard_flags: None,
        }
    }

    pub fn icons(&self) -> rune_md::icons::IconSet {
        self.icon_tier.markdown()
    }

    pub fn new_untitled(vfs: Arc<dyn Vfs + Send + Sync>, db: Option<Db>) -> App {
        let mut app = App::new(Buffer::new(""), None, vfs, db);
        app.active_doc_mut().display_name = Some("Untitled 1".to_string());
        app.splits.left.show();
        app
    }

    fn mint_doc_id(&mut self) -> DocumentId {
        let id = DocumentId(self.next_doc_id);
        self.next_doc_id = self.next_doc_id.saturating_add(1);
        id
    }

    pub fn open_document(&mut self, buffer: Buffer) -> DocumentId {
        let id = self.mint_doc_id();
        self.documents.insert(id, Document::new(buffer));
        id
    }

    pub fn doc(&self, id: DocumentId) -> Option<&Document> {
        self.documents.get(&id)
    }

    pub fn doc_mut(&mut self, id: DocumentId) -> Option<&mut Document> {
        self.documents.get_mut(&id)
    }

    pub fn active_doc(&self) -> &Document {
        self.documents.get_or_anchor(&self.active)
    }

    pub fn active_doc_mut(&mut self) -> &mut Document {
        self.documents.get_or_anchor_mut(&self.active)
    }

    pub fn is_dirty(&self) -> bool {
        self.active_doc().is_dirty()
    }

    pub fn is_preserved(&self, doc: &Document) -> bool {
        doc.is_store_bound() && self.db.as_ref().is_some_and(|db| !db.degraded)
    }

    pub fn file_name(&self) -> &str {
        self.active_doc().file_name()
    }

    pub fn set_root(&mut self, root: PathBuf) {
        self.root = Some(root);
    }

    pub fn frame_width(&self) -> u16 {
        self.frame.map_or(0, |frame| frame.width)
    }

    pub fn frame_height(&self) -> u16 {
        self.frame.map_or(0, |frame| frame.height)
    }

    pub fn frame_area(&self) -> ratatui::layout::Rect {
        ratatui::layout::Rect::new(0, 0, self.frame_width(), self.frame_height())
    }
}

pub fn update(app: &mut App, msg: Msg, effects: &mut Effects) {
    let journal_pos_before = app.active_doc().journal.pos();
    let active_before = app.active;
    let buffer_version_before = app.active_doc().buffer.version();
    let frame_width_before = app.frame_width();
    let focus_before = app.focus();
    let nav_index_before = app.nav_history.index();
    let nav_caret_before = app.active_doc().cursors.primary().position.get();
    dispatch::update_inner(app, msg, effects);
    let journal_pos_after = app.doc(active_before).map(|doc| doc.journal.pos());
    if journal_pos_after.is_some_and(|pos| pos != journal_pos_before) {
        save::schedule_snapshot_debounce(app, active_before);
    }
    if app.nav_history.index() == nav_index_before {
        crate::navhistory::observe_jump(app, active_before, nav_caret_before);
    }
    dispatch::after_update(
        app,
        active_before,
        buffer_version_before,
        frame_width_before,
        effects,
    );
    crate::explorer_preview::on_focus_changed(app, focus_before, app.focus());
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use rune_core::buffer::Buffer;
    use rune_vfs::{Mem, Vfs, VfsTestExt};

    use crate::db::{Db, DbBridge};
    use crate::keymap::{KeyCode, KeyInput, Mods};
    use crate::runtime::{Effects, Msg};
    use crate::workspace;

    use super::{App, update};

    fn drain_db_ops(app: &mut App, bridge: &DbBridge, effects: &mut Effects) {
        while !app.db_ops.is_empty() {
            let evt = bridge.wait_for_bootstrap_event(|_| true);
            update(app, Msg::Db(evt), effects);
        }
    }

    #[test]
    fn closing_the_active_document_never_arms_a_neighbors_snapshot_debounce() {
        let mem = Arc::new(Mem::new());
        mem.save_atomic(Path::new("/a.md"), b"a")
            .expect("seed a.md");
        mem.save_atomic(Path::new("/b.md"), b"b")
            .expect("seed b.md");
        let vfs: Arc<dyn Vfs + Send + Sync> = mem;
        let bridge = DbBridge::bootstrap();
        let clock: rune_db::ClockFn = Arc::new(std::time::SystemTime::now);
        let store = rune_db::Store::open_in_memory(clock, Arc::clone(&vfs), bridge.on_event())
            .expect("open store");
        let db = Db::new(store, Arc::clone(&bridge), false);
        let mut app = App::new(Buffer::new(""), None, vfs, Some(db));
        let mut effects = Effects::default();

        let a = workspace::open_path(&mut app, Path::new("/a.md")).expect("open a.md");
        drain_db_ops(&mut app, &bridge, &mut effects);
        let b = workspace::open_path(&mut app, Path::new("/b.md")).expect("open b.md");
        drain_db_ops(&mut app, &bridge, &mut effects);

        workspace::switch_to(&mut app, b);
        update(
            &mut app,
            Msg::Key(KeyInput {
                code: KeyCode::Char('x'),
                mods: Mods::NONE,
            }),
            &mut effects,
        );
        drain_db_ops(&mut app, &bridge, &mut effects);
        assert!(
            app.doc(b).expect("doc open").journal.pos() > 0,
            "test setup: b.md must have journal history distinct from a.md's"
        );
        let b_generation_before = app
            .doc(b)
            .expect("doc open")
            .doc_db()
            .expect("store-bound")
            .snapshot_generation;

        workspace::switch_to(&mut app, a);
        update(
            &mut app,
            Msg::Key(KeyInput {
                code: KeyCode::Char('w'),
                mods: Mods {
                    ctrl: true,
                    ..Mods::NONE
                },
            }),
            &mut effects,
        );

        assert!(app.doc(a).is_none(), "^w must close the clean active tab");
        assert_eq!(app.active, b, "b.md is the only remaining tab");
        assert_eq!(
            app.doc(b)
                .expect("doc open")
                .doc_db()
                .expect("store-bound")
                .snapshot_generation,
            b_generation_before,
            "closing a.md must not arm a snapshot debounce for b.md, \
             which this message never edited"
        );
    }
}
