//! The recovery-store bootstrap seam split out of `main` so that module can
//! stay focused on argument parsing plus the wiring that constructs the
//! `Vfs`, the store, and the runtime (plan WP4.S5/S1, re-split alongside
//! `AppDb` -> `Db`/`DocDb`, plan decision 5).

use std::path::Path;
use std::sync::Arc;

use rune_db::{DbEvent, OpOutcome, Store};
use rune_tui::db::{Db, DbBridge, DocDb};
use rune_vfs::Vfs;

/// The result of [`bootstrap_db`] — everything `bootstrap` needs to finish
/// constructing `App` with a hydrated recovery store (plan WP5.S2/S4,
/// re-split in WP1 alongside `AppDb` -> `Db`/`DocDb`, plan decision 5):
/// `db` wires onto `App` directly (`App::new`'s 4th argument); `doc_db`
/// installs on the initial document afterward, since `App::new` only knows
/// about the app-wide half.
#[derive(Default)]
pub(crate) struct DbBootstrap {
    pub(crate) db: Option<Db>,
    pub(crate) doc_db: Option<DocDb>,
    /// `Some` whenever `rune-db`'s `Load` returned reconstructed content
    /// (which may or may not differ from the buffer `load_buffer` already
    /// read straight off disk) — `main` runs this through the same
    /// `Document::hydrate` chokepoint `db::handle_load_ack` uses, once
    /// `App::new` exists to hold the result.
    pub(crate) recovered_content: Option<String>,
    /// The persistent degraded-store status banner (plan WP5.S2), or
    /// `None` when the store opened clean.
    pub(crate) banner: Option<String>,
}

/// One exit path for every "recovery store bootstrap failed after a `Store`
/// was actually opened" branch below (plan WP4.S5/[rune-cli 11] — these
/// used to be written out four times near-verbatim): prints the reason,
/// drains the writer thread, and returns the all-`None`/banner-set
/// bootstrap the editor runs with when recovery is unavailable.
fn degrade(store: Store, msg: impl Into<String>) -> DbBootstrap {
    let msg = msg.into();
    eprintln!("rune: recovery store degraded: {msg}");
    store.shutdown();
    DbBootstrap {
        banner: Some(format!("recovery disabled: {msg}")),
        ..DbBootstrap::default()
    }
}

/// Opens the recovery store at `$HOME/Library/Application Support/rune/
/// rune-v{SCHEMA_VERSION}.db` and hydrates `path` through it (plan
/// WP5.S2/S4), BEFORE the TUI ever starts (`runtime::run` hasn't been
/// called yet — no `Sender<Msg>` exists; see `db::DbBridge`'s doc comment
/// for why hydration blocks on the bridge's OWN buffer instead). Never
/// fatal to the editor: any failure here is reported to stderr and this
/// returns `DbBootstrap::default()` — the editor still opens and runs
/// fully, just without recovery journaling for this launch (CONSTITUTION
/// Prime Directive: the user's words come before every other feature, plan
/// decision 5: "losing the DB never damages a user file").
///
/// `home` is threaded in rather than read from `$HOME` directly (unlike
/// `rune_db::production_db_path`) so this whole path is exercisable
/// against a temp directory in tests (plan WP4.S1/S7) without touching the
/// real machine's recovery store.
pub(crate) fn bootstrap_db(
    vfs: Arc<dyn Vfs + Send + Sync>,
    path: &Path,
    home: Option<&Path>,
) -> DbBootstrap {
    let db_path = match home {
        Some(home) if !home.as_os_str().is_empty() => home
            .join("Library")
            .join("Application Support")
            .join("rune")
            .join(rune_db::db_file_name(rune_db::SCHEMA_VERSION)),
        _ => {
            return DbBootstrap {
                banner: Some("recovery disabled: $HOME not set".to_string()),
                ..DbBootstrap::default()
            };
        }
    };

    let bridge = DbBridge::bootstrap();
    let (store, open_warning) = match Store::open(&db_path, Arc::clone(&vfs), bridge.on_event()) {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("rune: recovery store unavailable: {e}");
            return DbBootstrap {
                banner: Some(format!("recovery disabled: {e}")),
                ..DbBootstrap::default()
            };
        }
    };
    let degraded_at_open = store.degraded();

    let load_op_id = match store.load(path) {
        Ok(id) => id,
        Err(e) => return degrade(store, format!("load failed: {e}")),
    };

    // Blocks main() — there is no runtime loop yet to be blocked instead
    // (`db::DbBridge`'s doc comment). Any event for a DIFFERENT op id can't
    // arrive yet (this is the very first op this `Store` has been asked to
    // run) — the predicate stays defensive rather than assuming it, and
    // leaves any such event buffered for `attach` rather than consuming it.
    // The writer thread always posts a `Fatal` before parking on a panic
    // (`writer.rs`'s own guarantee), so there is no "sender disconnected"
    // case left to handle here the way an `mpsc::Receiver` would need to.
    let load_outcome = match bridge.wait_for_bootstrap_event(|evt| match evt {
        DbEvent::Ok { id, .. } | DbEvent::Err { id, .. } => *id == load_op_id,
        DbEvent::Fatal { .. } => true,
    }) {
        DbEvent::Ok { result, .. } => Ok(result),
        DbEvent::Err { error, .. } => Err(error),
        DbEvent::Fatal { error } => Err(error),
    };

    let load_result = match load_outcome {
        Ok(OpOutcome::Load(load_result)) => *load_result,
        Ok(_) => {
            return degrade(store, "internal error: unexpected reply to Load");
        }
        Err(e) => return degrade(store, format!("load failed: {e}")),
    };

    // §1.7: `saved_obs` is `None` here only if `load` itself failed to
    // adopt anything for this session/doc pair — "should not occur" per
    // `LoadResult::saved_obs`'s own doc comment, but a `0` fallback would be
    // a fabricated `ObsId` (AUTOINCREMENT ids start at 1, so `0` is never a
    // real row) silently handed to every later CAS `materialize` as if it
    // were a genuine baseline. Treat it as the loud internal error it is —
    // degrade rather than fake a baseline no observation backs.
    let Some(expect_obs) = load_result.saved_obs else {
        return degrade(
            store,
            "internal error: load did not adopt a saved_obs baseline",
        );
    };

    let db = Db::new(store, bridge, degraded_at_open);
    let doc_db = DocDb::new(
        load_result.doc_id,
        expect_obs,
        false, // bind_new: `file_existed` at the call site guarantees the target exists
        // last_known_seq: `load` may have already durably journaled a
        // cross-session-inheritance bridge edit under THIS session's own
        // id — `bridge_seq` is that edit's own seq when it happened, and
        // this session's true durable journal head either way (a fresh
        // session journals nothing else during `load`). `0` would silently
        // regress behind it for any `move_undo_pos`/`materialize` issued
        // before the first ordinary `AppendEdit` ack lands (finding 8).
        load_result.bridge_seq.unwrap_or(0),
    );

    let banner = if degraded_at_open {
        Some(open_warning.unwrap_or_else(|| rune_db::DEGRADED_WARNING.to_string()))
    } else {
        None
    };

    DbBootstrap {
        db: Some(db),
        doc_db: Some(doc_db),
        recovered_content: Some(load_result.recovered),
        banner,
    }
}
