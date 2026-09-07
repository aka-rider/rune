use std::path::{Path, PathBuf};
use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_db::{ClockFn, DbEvent, OpOutcome, Store, SyncKind};
use rune_vfs::{Mem, Vfs, VfsTestExt};

use crate::app::App;
use crate::db::{Db, DbBridge};
use crate::document::{DocumentId, Replica};
use crate::find::test_support::{
    CTRL, PROJECT_H, PROJECT_W, key, press_into, search_project, shift_enter, type_into,
};
use crate::keymap::KeyCode;
use crate::messages;
use crate::runtime::{Effects, Msg};
use crate::workspace;

pub(crate) const A: &str = "/root/a.md";
pub(crate) const B: &str = "/root/b.md";
pub(crate) const C: &str = "/root/c.md";

pub(crate) const FILES: &[(&str, &str)] =
    &[(A, "dog one dog"), (B, "a dog here"), (C, "Dog and dog")];

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum StoreKind {
    Absent,
    Degraded,
    Silent,
    Live,
}

pub(crate) struct Fixture {
    pub app: App,
    pub mem: Arc<Mem>,
    pub bridge: Option<Arc<DbBridge>>,
    pub effects: Effects,
}

pub(crate) fn fixture(store: StoreKind) -> Fixture {
    fixture_with(store, None)
}

pub(crate) fn fixture_with(kind: StoreKind, launch: Option<&str>) -> Fixture {
    let mem = Arc::new(Mem::new());
    for (path, content) in FILES {
        mem.save_atomic(Path::new(path), content.as_bytes())
            .expect("seed file");
    }
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(&mem) as Arc<dyn Vfs + Send + Sync>;
    let clock: ClockFn = Arc::new(std::time::SystemTime::now);
    let (db, bridge) = match kind {
        StoreKind::Absent => (None, None),
        StoreKind::Degraded | StoreKind::Silent => {
            let store = Store::open_in_memory(clock, Arc::clone(&vfs), Box::new(|_evt| {}))
                .expect("open store");
            let db = Db::new(store, DbBridge::bootstrap(), kind == StoreKind::Degraded);
            (Some(db), None)
        }
        StoreKind::Live => {
            let bridge = DbBridge::bootstrap();
            let store = Store::open_in_memory(clock, Arc::clone(&vfs), bridge.on_event())
                .expect("open store");
            (
                Some(Db::new(store, Arc::clone(&bridge), false)),
                Some(bridge),
            )
        }
    };
    let (buffer, launch) = match launch {
        Some(path) => {
            mem.save_atomic(Path::new(path), b"origin text")
                .expect("seed origin");
            let resolved = crate::resolved::ResolvedPath::resolve(vfs.as_ref(), Path::new(path))
                .expect("the launch path resolves");
            (Buffer::new("origin text"), Some(resolved))
        }
        None => (Buffer::new("hello"), None),
    };
    let mut app = App::new(buffer, launch, vfs, db);
    app.frame = Some(crate::app::FrameSize::new(PROJECT_W, PROJECT_H));
    app.root = Some(PathBuf::from("/root"));
    app.sync_view();
    Fixture {
        app,
        mem,
        bridge,
        effects: Effects::default(),
    }
}

impl Fixture {
    pub(crate) fn prepare(&mut self, query: &str, replacement: &str) {
        search_project(&mut self.app, query, &mut self.effects);
        press_into(
            &mut self.app,
            key(KeyCode::Char('R'), CTRL),
            &mut self.effects,
        );
        type_into(&mut self.app, replacement, &mut self.effects);
    }

    pub(crate) fn replace_all(&mut self) {
        press_into(&mut self.app, shift_enter(), &mut self.effects);
    }

    pub(crate) fn disk(&self, path: &str) -> String {
        String::from_utf8(self.mem.read(Path::new(path)).expect("file on disk"))
            .expect("utf-8 on disk")
    }

    pub(crate) fn doc_for(&self, path: &str) -> Option<DocumentId> {
        workspace::existing_document_for_spelling(&self.app, Path::new(path))
    }

    pub(crate) fn content_of(&self, path: &str) -> String {
        let id = self.doc_for(path).expect("the file is open");
        self.app
            .doc(id)
            .expect("doc exists")
            .buffer
            .content()
            .to_string()
    }

    pub(crate) fn is_binding(&self, path: &str) -> bool {
        let id = self.doc_for(path).expect("the file is open");
        matches!(
            self.app.doc(id).expect("doc exists").replica,
            Replica::Binding { .. }
        )
    }

    pub(crate) fn pending_load_op(&self, id: DocumentId) -> u64 {
        *self
            .app
            .db_ops
            .iter()
            .find(|(_, pending)| pending.doc == id && pending.issued_version.is_some())
            .expect("a load op is in flight for the document")
            .0
    }

    pub(crate) fn ack_load(&mut self, path: &str, recovered: &str, kind: SyncKind) {
        let id = self.doc_for(path).expect("the file is open");
        let op_id = self.pending_load_op(id);
        let db_id = i64::try_from(id.0.get()).expect("small id");
        let load_result = rune_db::LoadResult {
            doc_id: rune_db::DocId(db_id),
            renamed_from: None,
            disk_content: self.disk(path),
            recovered: rune_db::Recovered {
                content: recovered.to_string(),
                cursors: Vec::new(),
            },
            has_history: true,
            sync: rune_db::SyncState {
                kind,
                ancestor: None,
                ours: rune_db::Version {
                    hash: rune_db::BlobHash(String::new()),
                    obs: None,
                },
                theirs: None,
            },
            nlink: 1,
            saved_obs: rune_db::ObsId::new(db_id),
            bridge_seq: None,
            resumable_merge: None,
        };
        crate::app::update(
            &mut self.app,
            Msg::Db(DbEvent::Ok {
                id: op_id,
                result: OpOutcome::Load(Box::new(load_result)),
            }),
            &mut self.effects,
        );
    }

    pub(crate) fn ack_clean_load(&mut self, path: &str) {
        let disk = self.disk(path);
        self.ack_load(path, &disk, SyncKind::Clean);
    }

    pub(crate) fn deliver_next_real_load_ack(&mut self) -> DocumentId {
        let evt = self
            .bridge
            .as_ref()
            .expect("a live store")
            .wait_for_bootstrap_event(|evt| {
                matches!(
                    evt,
                    DbEvent::Ok {
                        result: OpOutcome::Load(_),
                        ..
                    }
                )
            });
        let DbEvent::Ok { id: op_id, .. } = &evt else {
            unreachable!("the predicate only admits Ok events");
        };
        let doc = self
            .app
            .db_ops
            .get(op_id)
            .expect("the ack answers a recorded op")
            .doc;
        crate::app::update(&mut self.app, Msg::Db(evt), &mut self.effects);
        doc
    }

    pub(crate) fn open_extra_docs(&mut self, count: usize, pinned: bool) -> Vec<DocumentId> {
        (0..count)
            .map(|i| {
                let path = crate::resolved::ResolvedPath::resolve(
                    self.app.vfs.as_ref(),
                    Path::new(&format!("/other/p{i}.md")),
                )
                .expect("Mem resolves any spelling");
                let id = self.app.open_document_bound(Buffer::new("filler"), path);
                if let Some(doc) = self.app.doc_mut(id) {
                    doc.pinned = pinned;
                }
                id
            })
            .collect()
    }

    pub(crate) fn newest(&self) -> &str {
        messages::newest_text(&self.app).unwrap_or_default()
    }

    pub(crate) fn seed_content(path: &str) -> &'static str {
        FILES
            .iter()
            .find(|(seeded, _)| *seeded == path)
            .map(|(_, content)| *content)
            .expect("a seeded hit file")
    }
}

pub(crate) fn assert_disk_untouched(fx: &Fixture) {
    for (path, content) in FILES {
        assert_eq!(fx.disk(path), *content, "{path} must not change on disk");
    }
}

pub(crate) fn assert_all_replaced(fx: &Fixture) {
    assert_eq!(fx.content_of(A), "cat one cat");
    assert_eq!(fx.content_of(B), "a cat here");
    assert_eq!(fx.content_of(C), "cat and cat");
}
