use std::collections::{HashMap, HashSet};
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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct QuitIntent {
    pub pending: std::collections::BTreeMap<DocumentId, u64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum QuitNegotiation {
    #[default]
    Idle,
    ConfirmArmed(QuitKey, crate::generation::Generation),
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
    pub frame_height: u16,
    pub frame_width: u16,
    pub explorer: Explorer,
    pub tabs: OpenTabs,
    pub help_doc: Option<DocumentId>,
    pub help_return_to: Option<DocumentId>,
    pub title: crate::title::TitleField,
    pub rename: crate::rename::RenameState,
    pub(crate) next_rename_gen: crate::generation::GenCounter,
    pub merge: crate::merge::MergeState,
    pub(crate) next_merge_gen: crate::generation::GenCounter,
    pub diff: Option<crate::diff_view::DiffView>,
    pub db: Option<Db>,
    pub db_ops: HashMap<u64, crate::db::PendingOp>,
    pub file_bindings: HashMap<i64, crate::db::FileBinding>,
    pub db_banner: Option<String>,
    pub pending_save_confirm: Option<(DocumentId, crate::generation::Generation)>,
    pub(crate) next_save_confirm_gen: crate::generation::GenCounter,
    pub pending_close_on_save: Option<DocumentId>,
    pub quit: QuitNegotiation,
    pub(crate) next_quit_gen: crate::generation::GenCounter,
    pub pointer: crate::pointer::PointerState,
    pub clock: Arc<dyn crate::pointer::Clock + Send + Sync>,
    pub binding_set: crate::keymap::BindingSet,
    pub guard: Option<GuardPrompt>,
    pub(crate) trash_gen: crate::generation::Generation,
    pub(crate) next_trash_gen: crate::generation::GenCounter,
    pub(crate) trash_pending: Option<PathBuf>,
    pub messages: MessageLog,
    pub(crate) overlay: crate::overlay::Overlay,
    pub(crate) next_filesearch_gen: crate::generation::GenCounter,
    pub(crate) last_search_query: Option<String>,
    pub(crate) last_persisted_search_query: Option<String>,
    pub(crate) next_search_history_gen: crate::generation::GenCounter,
    pub(crate) search_history_ops: HashSet<u64>,
    pub(crate) next_palette_gen: crate::generation::GenCounter,
    pub(crate) last_persisted_command: Option<String>,
    pub(crate) command_history_ops: HashSet<u64>,
    pub should_quit: bool,
    pub theme: crate::theme::Theme,
    pub icon_tier: crate::theme::icons::IconTier,
    pub graphics: crate::graphics::GraphicsCaps,
    pub root: Option<PathBuf>,
    pub(crate) timers: Arc<crate::runtime::TimerService>,
    pub nav_history: crate::navhistory::NavHistory,
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
            frame_height: 0,
            frame_width: 0,
            explorer: Explorer::default(),
            tabs: OpenTabs::new(),
            help_doc: None,
            help_return_to: None,
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
            trash_gen: crate::generation::Generation::ZERO,
            next_trash_gen: crate::generation::GenCounter::default(),
            trash_pending: None,
            messages: MessageLog::new(),
            overlay: crate::overlay::Overlay::None,
            next_filesearch_gen: crate::generation::GenCounter::default(),
            last_search_query: None,
            last_persisted_search_query: None,
            next_search_history_gen: crate::generation::GenCounter::default(),
            search_history_ops: HashSet::new(),
            next_palette_gen: crate::generation::GenCounter::default(),
            last_persisted_command: None,
            command_history_ops: HashSet::new(),
            should_quit: false,
            theme: crate::theme::Theme::catppuccin_mocha(false),
            icon_tier: crate::theme::icons::IconTier::Unicode,
            graphics: crate::graphics::GraphicsCaps::default(),
            root: None,
            timers: crate::runtime::TimerService::new(),
            nav_history: crate::navhistory::NavHistory::default(),
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
}

pub fn update(app: &mut App, msg: Msg, effects: &mut Effects) {
    let journal_pos_before = app.active_doc().journal.pos();
    let active_before = app.active;
    let buffer_version_before = app.active_doc().buffer.version();
    let focus_before = app.focus();
    let nav_index_before = app.nav_history.index();
    let nav_caret_before = app.active_doc().cursors.primary().position;
    dispatch::update_inner(app, msg, effects);
    if app.active_doc().journal.pos() != journal_pos_before {
        let id = app.active;
        save::schedule_snapshot_debounce(app, id);
    }
    if app.nav_history.index() == nav_index_before {
        crate::navhistory::observe_jump(app, active_before, nav_caret_before);
    }
    dispatch::after_update(app, active_before, buffer_version_before, effects);
    crate::explorer_preview::on_focus_changed(app, focus_before, app.focus());
}
