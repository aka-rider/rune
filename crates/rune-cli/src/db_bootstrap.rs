
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rune_db::{DbEvent, OpOutcome, Store};
use rune_tui::db::{Db, DbBridge, DocDb, PublishMode};
use rune_vfs::Vfs;

#[derive(Default)]
pub(crate) struct DbBootstrap {
    pub(crate) db: Option<Db>,
    pub(crate) doc_db: Option<DocDb>,
    pub(crate) expect_obs: Option<rune_db::ObsId>,
    pub(crate) recovered_content: Option<rune_db::Recovered>,
    pub(crate) sync_kind: Option<rune_db::SyncKind>,
    pub(crate) nlink: Option<i64>,
    pub(crate) banner: Option<String>,
}

fn db_path_for(home: Option<&Path>) -> Option<PathBuf> {
    match home {
        Some(home) if !home.as_os_str().is_empty() => Some(
            home.join("Library")
                .join("Application Support")
                .join("rune")
                .join(rune_db::db_file_name(rune_db::SCHEMA_VERSION)),
        ),
        _ => None,
    }
}

struct OpenedStore {
    bridge: Arc<DbBridge>,
    store: Store,
    degraded_at_open: bool,
    warning: Option<String>,
}

impl From<String> for DbBootstrap {
    fn from(banner: String) -> Self {
        DbBootstrap {
            banner: Some(banner),
            ..DbBootstrap::default()
        }
    }
}

impl From<String> for DbBootstrapUntitled {
    fn from(banner: String) -> Self {
        DbBootstrapUntitled {
            banner: Some(banner),
            ..DbBootstrapUntitled::default()
        }
    }
}

fn open_store(vfs: Arc<dyn Vfs + Send + Sync>, home: Option<&Path>) -> Result<OpenedStore, String> {
    let Some(db_path) = db_path_for(home) else {
        return Err("recovery disabled: $HOME not set".to_string());
    };

    let bridge = DbBridge::bootstrap();
    let (store, warning) = match Store::open(&db_path, vfs, bridge.on_event()) {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("rune: recovery store unavailable: {e}");
            return Err(format!("recovery disabled: {e}"));
        }
    };
    let degraded_at_open = store.degraded();

    Ok(OpenedStore {
        bridge,
        store,
        degraded_at_open,
        warning,
    })
}

fn degrade(store: Store, msg: impl Into<String>) -> DbBootstrap {
    let msg = msg.into();
    eprintln!("rune: recovery store degraded: {msg}");
    store.shutdown();
    DbBootstrap {
        banner: Some(format!("recovery disabled: {msg}")),
        ..DbBootstrap::default()
    }
}

fn blocking_call(
    bridge: &DbBridge,
    enqueue: impl FnOnce() -> Result<u64, rune_db::Error>,
) -> Result<OpOutcome, String> {
    let op_id = enqueue().map_err(|e| e.to_string())?;
    match bridge.wait_for_bootstrap_event(|evt| match evt {
        DbEvent::Ok { id, .. } | DbEvent::Err { id, .. } => *id == op_id,
        DbEvent::Fatal { .. } => true,
    }) {
        DbEvent::Ok { result, .. } => Ok(result),
        DbEvent::Err { error, .. } | DbEvent::Fatal { error } => Err(error),
    }
}

pub(crate) fn bootstrap_db(
    vfs: Arc<dyn Vfs + Send + Sync>,
    path: &Path,
    home: Option<&Path>,
    sighting: rune_vfs::Sighting,
) -> DbBootstrap {
    let OpenedStore {
        bridge,
        store,
        degraded_at_open,
        warning: open_warning,
    } = match open_store(vfs, home) {
        Ok(opened) => opened,
        Err(banner) => return banner.into(),
    };

    let load_outcome = blocking_call(&bridge, || store.load_sighted(path, sighting));

    let load_result = match load_outcome {
        Ok(OpOutcome::Load(load_result)) => *load_result,
        Ok(_) => {
            return degrade(store, "internal error: unexpected reply to Load");
        }
        Err(e) => return degrade(store, format!("load failed: {e}")),
    };

    let Some(expect_obs) = load_result.saved_obs else {
        return degrade(
            store,
            "internal error: load did not adopt a saved_obs baseline",
        );
    };

    let db = Db::new(store, bridge, degraded_at_open);
    let doc_db = DocDb::new(
        load_result.doc_id.0,
        PublishMode::OverwriteExisting,
        load_result.bridge_seq.unwrap_or(rune_db::Seq(0)),
    );

    let banner = if degraded_at_open {
        Some(open_warning.unwrap_or_else(|| rune_db::DEGRADED_WARNING.to_string()))
    } else {
        None
    };

    let sync_kind = load_result.sync.kind;

    DbBootstrap {
        db: Some(db),
        doc_db: Some(doc_db),
        expect_obs: Some(expect_obs),
        recovered_content: Some(load_result.recovered),
        sync_kind: Some(sync_kind),
        nlink: Some(load_result.nlink),
        banner,
    }
}

pub(crate) struct ScratchDoc {
    pub(crate) db_id: i64,
    pub(crate) recovered: rune_db::Recovered,
}

#[derive(Default)]
pub(crate) struct DbBootstrapUntitled {
    pub(crate) db: Option<Db>,
    pub(crate) scratch_docs: Vec<ScratchDoc>,
    pub(crate) banner: Option<String>,
}

fn degrade_untitled(store: Store, msg: impl Into<String>) -> DbBootstrapUntitled {
    let msg = msg.into();
    eprintln!("rune: recovery store degraded: {msg}");
    store.shutdown();
    DbBootstrapUntitled {
        banner: Some(format!("recovery disabled: {msg}")),
        ..DbBootstrapUntitled::default()
    }
}

pub(crate) fn bootstrap_untitled_db(
    vfs: Arc<dyn Vfs + Send + Sync>,
    home: Option<&Path>,
) -> DbBootstrapUntitled {
    let OpenedStore {
        bridge,
        store,
        degraded_at_open,
        warning: open_warning,
    } = match open_store(vfs, home) {
        Ok(opened) => opened,
        Err(banner) => return banner.into(),
    };

    let recoverable_ids = match blocking_call(&bridge, || store.recoverable_scratch(0)) {
        Ok(OpOutcome::Ids(ids)) => ids,
        Ok(_) => {
            return degrade_untitled(
                store,
                "internal error: unexpected reply to RecoverableScratch",
            );
        }
        Err(e) => return degrade_untitled(store, format!("recoverable scratch failed: {e}")),
    };

    let mut scratch_docs = Vec::new();
    for db_id in recoverable_ids {
        match blocking_call(&bridge, || store.reconstruct_scratch(rune_db::DocId(db_id))) {
            Ok(OpOutcome::Reconstructed(Some(recovered)))
                if !recovered.content.trim().is_empty() =>
            {
                scratch_docs.push(ScratchDoc { db_id, recovered });
            }
            Ok(OpOutcome::Reconstructed(_)) => {}
            Ok(_) => {
                return degrade_untitled(
                    store,
                    "internal error: unexpected reply to ReconstructScratch",
                );
            }
            Err(e) => return degrade_untitled(store, format!("reconstruct scratch failed: {e}")),
        }
    }

    if scratch_docs.is_empty() {
        match blocking_call(&bridge, || store.create_scratch()) {
            Ok(OpOutcome::ScratchDocId(id)) => scratch_docs.push(ScratchDoc {
                db_id: id.0,
                recovered: rune_db::Recovered::default(),
            }),
            Ok(_) => {
                return degrade_untitled(
                    store,
                    "internal error: unexpected reply to CreateScratch",
                );
            }
            Err(e) => return degrade_untitled(store, format!("create scratch failed: {e}")),
        }
    }

    if let Some(keep) = scratch_docs.first()
        && let Err(e) = blocking_call(&bridge, || store.gc_empty_scratch(keep.db_id))
    {
        eprintln!("rune: gc_empty_scratch failed (non-fatal): {e}");
    }

    let db = Db::new(store, bridge, degraded_at_open);
    let banner = if degraded_at_open {
        Some(open_warning.unwrap_or_else(|| rune_db::DEGRADED_WARNING.to_string()))
    } else {
        None
    };

    DbBootstrapUntitled {
        db: Some(db),
        scratch_docs,
        banner,
    }
}

pub(crate) fn bootstrap_new_file(
    vfs: Arc<dyn Vfs + Send + Sync>,
    path: &Path,
    home: Option<&Path>,
) -> DbBootstrap {
    let OpenedStore {
        bridge,
        store,
        degraded_at_open,
        warning: open_warning,
    } = match open_store(vfs, home) {
        Ok(opened) => opened,
        Err(banner) => return banner.into(),
    };

    let intended_path = path.to_string_lossy().into_owned();
    let inherited = match find_named_draft(&bridge, &store, &intended_path) {
        Ok(inherited) => inherited,
        Err(e) => return degrade(store, e),
    };

    let (db_id, recovered_content) = match inherited {
        Some((db_id, content)) => (db_id, Some(content)),
        None => {
            let db_id = match blocking_call(&bridge, || store.create_named_scratch(&intended_path))
            {
                Ok(OpOutcome::ScratchDocId(id)) => id.0,
                Ok(_) => {
                    return degrade(store, "internal error: unexpected reply to CreateScratch");
                }
                Err(e) => return degrade(store, format!("create scratch failed: {e}")),
            };
            (db_id, None)
        }
    };

    let db = Db::new(store, bridge, degraded_at_open);
    let doc_db = DocDb::new(db_id, PublishMode::CreateOnly, rune_db::Seq(0));
    let banner = if degraded_at_open {
        Some(open_warning.unwrap_or_else(|| rune_db::DEGRADED_WARNING.to_string()))
    } else {
        None
    };

    DbBootstrap {
        db: Some(db),
        doc_db: Some(doc_db),
        expect_obs: None,
        recovered_content,
        sync_kind: None,
        nlink: None,
        banner,
    }
}

fn find_named_draft(
    bridge: &DbBridge,
    store: &Store,
    intended_path: &str,
) -> Result<Option<(i64, rune_db::Recovered)>, String> {
    let candidate_ids = match blocking_call(bridge, || store.find_named_scratch(intended_path)) {
        Ok(OpOutcome::Ids(ids)) => ids,
        Ok(_) => {
            return Err("internal error: unexpected reply to FindNamedScratch".to_string());
        }
        Err(e) => return Err(format!("find named scratch failed: {e}")),
    };

    for db_id in candidate_ids {
        match blocking_call(bridge, || store.reconstruct_scratch(rune_db::DocId(db_id))) {
            Ok(OpOutcome::Reconstructed(Some(recovered)))
                if !recovered.content.trim().is_empty() =>
            {
                return Ok(Some((db_id, recovered)));
            }
            Ok(OpOutcome::Reconstructed(_)) => {}
            Ok(_) => {
                return Err("internal error: unexpected reply to ReconstructScratch".to_string());
            }
            Err(e) => return Err(format!("reconstruct scratch failed: {e}")),
        }
    }
    Ok(None)
}

pub(crate) fn bootstrap_store_only(
    vfs: Arc<dyn Vfs + Send + Sync>,
    home: Option<&Path>,
) -> DbBootstrap {
    let OpenedStore {
        bridge,
        store,
        degraded_at_open,
        warning: open_warning,
    } = match open_store(vfs, home) {
        Ok(opened) => opened,
        Err(banner) => return banner.into(),
    };

    let db = Db::new(store, bridge, degraded_at_open);
    let banner = if degraded_at_open {
        Some(open_warning.unwrap_or_else(|| rune_db::DEGRADED_WARNING.to_string()))
    } else {
        None
    };

    DbBootstrap {
        db: Some(db),
        banner,
        ..DbBootstrap::default()
    }
}
