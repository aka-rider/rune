//! The recovery-store bootstrap seam split out of `main` so that module can
//! stay focused on argument parsing plus the wiring that constructs the
//! `Vfs`, the store, and the runtime (plan WP4.S5/S1, re-split alongside
//! `AppDb` -> `Db`/`DocDb`, plan decision 5).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rune_db::{DbEvent, OpOutcome, Store};
use rune_tui::db::{Db, DbBridge, DocDb};
use rune_vfs::Vfs;

/// The result of [`bootstrap_db`] (an existing path, loaded off disk) or
/// [`bootstrap_new_file`] (a positional naming a path that doesn't exist
/// yet, bound to a fresh scratch row instead) — everything `bootstrap`
/// needs to finish constructing `App` with a hydrated recovery store (plan
/// WP5.S2/S4, re-split in WP1 alongside `AppDb` -> `Db`/`DocDb`, plan
/// decision 5): `db` wires onto `App` directly (`App::new`'s 4th
/// argument); `doc_db` installs on the initial document afterward, since
/// `App::new` only knows about the app-wide half.
#[derive(Default)]
pub(crate) struct DbBootstrap {
    pub(crate) db: Option<Db>,
    pub(crate) doc_db: Option<DocDb>,
    /// The CAS baseline `main` joins `App::file_bindings` with for `doc_db`'s
    /// `db_id`, the same shared-per-file entry every later document bound to
    /// this `db_id` reads and advances — never installed directly on
    /// `doc_db` itself. `Some` alongside `doc_db` on every
    /// construction path: a genuine baseline for a real `Load`, or `0` (the
    /// same fabricated, never-queried value `doc_db`'s own `bind_new: true`
    /// paths use) for a fresh scratch row.
    pub(crate) expect_obs: Option<rune_db::ObsId>,
    /// `Some` whenever `rune-db`'s `Load` returned reconstructed content
    /// (which may or may not differ from the buffer `load_sighting` already
    /// read straight off disk) — `main` runs this through the same
    /// `Document::hydrate` chokepoint `db::handle_load_ack` uses, once
    /// `App::new` exists to hold the result.
    pub(crate) recovered_content: Option<String>,
    /// This `Load`'s [`rune_db::SyncKind`] (see `Document::last_sync`'s
    /// own doc comment) — render/hint state only, `main` installs it onto
    /// the active document the same way `db_ack::handle_load_ack` does for
    /// every later per-document reload. `None` whenever `rune_db::load`
    /// itself never ran: every early-return branch above `bootstrap_db`'s
    /// `Store::load` call, AND every successful [`bootstrap_new_file`]
    /// outcome — a missing-path launch binds a fresh scratch row instead of
    /// loading anything, so there is no `Load` on that path at all, not
    /// even on success.
    pub(crate) sync_kind: Option<rune_db::SyncKind>,
    /// The persistent degraded-store status banner (plan WP5.S2), or
    /// `None` when the store opened clean.
    pub(crate) banner: Option<String>,
}

/// Resolves the recovery store's file path from `$HOME` (threaded in rather
/// than read from the environment directly, so this is exercisable against
/// a temp directory in tests) — the one place [`open_store`] (in turn
/// shared by every bootstrap shape) computes it, so no launch shape can
/// ever disagree on where the one database file lives.
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

/// Everything a live [`Store`] hands back once the open ladder has run,
/// shared by every bootstrap shape that needs one open before doing its own
/// store work ([`bootstrap_db`], [`bootstrap_untitled_db`],
/// [`bootstrap_new_file`]).
struct OpenedStore {
    bridge: Arc<DbBridge>,
    store: Store,
    degraded_at_open: bool,
    warning: Option<String>,
}

/// Runs the shared open ladder (`db_path_for` -> `DbBridge::bootstrap` ->
/// `Store::open` -> `store.degraded()`) once, instead of writing it out per
/// bootstrap shape. `Err` is the ready-to-use banner text, already carrying
/// its own `recovery disabled:` prefix. The `Store::open` arm keeps writing
/// its `rune: recovery store unavailable: {e}` stderr line here; the
/// `$HOME`-unset arm stays silent, exactly as both did before this
/// extraction.
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

/// Enqueues one op via `enqueue` and blocks THIS thread (there is no runtime
/// loop yet — see [`DbBridge`]'s own doc comment) until its completion
/// arrives, returning the domain result or a flattened error message. Shared
/// by every bootstrap-time op (`Load`, and WP3's `RecoverableScratch`/
/// `ReconstructScratch`/`CreateScratch`/`GcEmptyScratch`) so this
/// enqueue-then-block shape is written once, not once per op kind.
fn blocking_call(
    bridge: &DbBridge,
    enqueue: impl FnOnce() -> Result<u64, rune_db::Error>,
) -> Result<OpOutcome, String> {
    let op_id = enqueue().map_err(|e| e.to_string())?;
    // Any event for a DIFFERENT op id can't arrive yet unless it was
    // enqueued by an earlier bootstrap step still buffered here — the
    // predicate stays defensive rather than assuming FIFO order across
    // calls, and leaves any such event buffered for `attach` rather than
    // consuming it. The writer thread always posts a `Fatal` before parking
    // on a panic (`writer.rs`'s own guarantee), so there is no "sender
    // disconnected" case left to handle here the way an `mpsc::Receiver`
    // would need to.
    match bridge.wait_for_bootstrap_event(|evt| match evt {
        DbEvent::Ok { id, .. } | DbEvent::Err { id, .. } => *id == op_id,
        DbEvent::Fatal { .. } => true,
    }) {
        DbEvent::Ok { result, .. } => Ok(result),
        DbEvent::Err { error, .. } => Err(error),
        DbEvent::Fatal { error } => Err(error),
    }
}

/// Opens the recovery store at `$HOME/Library/Application Support/rune/
/// rune-v{SCHEMA_VERSION}.db` and hydrates `path` through it (plan
/// WP5.S2/S4), BEFORE the TUI ever starts (`runtime::run` hasn't been
/// called yet — no `Sender<Msg>` exists; see `db::DbBridge`'s doc comment
/// for why hydration blocks on the bridge's OWN buffer instead). Never
/// fatal to the editor: any failure here is reported to stderr and this
/// returns `DbBootstrap::default()` — the editor still opens and runs
/// fully, just without recovery journaling for this launch (the user's
/// words come before every other feature, plan decision 5: "losing the DB
/// never damages a user file").
///
/// `home` is threaded in rather than read from `$HOME` directly (unlike
/// `rune_db::production_db_path`) so this whole path is exercisable
/// against a temp directory in tests (plan WP4.S1/S7) without touching the
/// real machine's recovery store.
///
/// `sighting` is the SAME [`rune_vfs::Sighting`] the caller already took of
/// `path` to build the initial buffer (issue #77) — threaded through to
/// `Store::load_sighted` so the CAS baseline this launch adopts traces to
/// that one read, never a second, independent one this function would
/// otherwise take itself.
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
        Err(banner) => {
            return DbBootstrap {
                banner: Some(banner),
                ..DbBootstrap::default()
            };
        }
    };

    let load_outcome = blocking_call(&bridge, || store.load_sighted(path, sighting));

    let load_result = match load_outcome {
        Ok(OpOutcome::Load(load_result)) => *load_result,
        Ok(_) => {
            return degrade(store, "internal error: unexpected reply to Load");
        }
        Err(e) => return degrade(store, format!("load failed: {e}")),
    };

    // `saved_obs` is `None` here only if `load` itself failed to
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
        false, // bind_new: the caller only reaches here when `sighting` confirms the target exists
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

    let sync_kind = load_result.sync.kind;

    DbBootstrap {
        db: Some(db),
        doc_db: Some(doc_db),
        expect_obs: Some(expect_obs),
        recovered_content: Some(load_result.recovered),
        sync_kind: Some(sync_kind),
        banner,
    }
}

/// One scratch document the no-positional launch will surface as a tab:
/// either a genuinely recovered draft (`db_id` names an EXISTING row, `rune-
/// db`'s `RecoverableScratch`/`ReconstructScratch`) or a brand-new one
/// (`content` empty, `db_id` a freshly minted row). Recovered drafts adopt
/// their OWN row rather than a fresh one copying the text in (plan WP3):
/// the source row keeps its events, `GcEmptyScratch` will not remove it (its
/// `inode IS NULL` filter is unconditional, not "only when empty"), and it
/// would otherwise be re-offered on every later launch forever.
pub(crate) struct ScratchDoc {
    pub(crate) db_id: i64,
    pub(crate) content: String,
}

/// The result of [`bootstrap_untitled_db`] — the no-positional-launch
/// counterpart to [`DbBootstrap`]. `scratch_docs` is non-empty exactly when
/// `db` is `Some`: a live store always yields at least one scratch document
/// (a recovered draft, or a freshly minted empty one when there was nothing
/// to recover), newest first — `main` adopts `scratch_docs[0]` onto the
/// already-constructed default document and opens the rest as their own
/// tabs.
#[derive(Default)]
pub(crate) struct DbBootstrapUntitled {
    pub(crate) db: Option<Db>,
    pub(crate) scratch_docs: Vec<ScratchDoc>,
    pub(crate) banner: Option<String>,
}

/// One exit path for every "store opened, but a later WP3 op failed" branch
/// below — mirrors [`degrade`], but for [`DbBootstrapUntitled`]'s shape (no
/// `doc_db`/`recovered_content` fields to leave `None`).
fn degrade_untitled(store: Store, msg: impl Into<String>) -> DbBootstrapUntitled {
    let msg = msg.into();
    eprintln!("rune: recovery store degraded: {msg}");
    store.shutdown();
    DbBootstrapUntitled {
        banner: Some(format!("recovery disabled: {msg}")),
        ..DbBootstrapUntitled::default()
    }
}

/// Opens the recovery store for a no-positional launch (plan WP3, "the
/// untitled draft is really recovery-backed") and makes the default draft
/// genuinely recovery-backed: unlike [`bootstrap_db`], there is no on-disk
/// path to `Load` — instead this lists every genuinely recoverable scratch
/// row left by a prior, now-dead session (`Store::recoverable_scratch`),
/// reconstructs each one's content across the session boundary
/// (`Store::reconstruct_scratch`, skipping empty/whitespace-only drafts and
/// ones whose owning session turns out still alive), and — only when NONE
/// were found — mints a brand-new scratch row (`Store::create_scratch`).
/// Either way, empty leftover scratch rows from prior sessions are swept
/// (`Store::gc_empty_scratch`) before returning, keeping whichever row this
/// session is about to adopt as its own default document.
///
/// Never fatal to the editor, exactly like [`bootstrap_db`]: any failure
/// here is reported to stderr and this returns [`DbBootstrapUntitled::
/// default`] — the editor still opens with a plain, non-recovery-backed
/// draft, precisely today's behaviour before this plan.
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
        Err(banner) => {
            return DbBootstrapUntitled {
                banner: Some(banner),
                ..DbBootstrapUntitled::default()
            };
        }
    };

    // `exclude_id: 0` — this is a fresh launch; no document row this
    // session cares about yet exists to exclude.
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
        match blocking_call(&bridge, || store.reconstruct_scratch(db_id)) {
            Ok(OpOutcome::Reconstructed(Some(content))) if !content.trim().is_empty() => {
                scratch_docs.push(ScratchDoc { db_id, content });
            }
            // No prior session ever touched it, its owning session is still
            // alive, or its reconstruction is empty/whitespace-only — never
            // offered as a recoverable tab.
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
            Ok(OpOutcome::RowId(db_id)) => scratch_docs.push(ScratchDoc {
                db_id,
                content: String::new(),
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

    // Keep whichever row this session is about to adopt as its own active
    // default document (the newest, first entry — `scratch_docs` is never
    // empty here, either branch above always pushed at least one) — GC is
    // fire-and-forget housekeeping; a failure here degrades nothing this
    // launch needs.
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

/// The launch counterpart of a recovery-backed untitled draft, for a
/// positional naming a file that does not exist yet: there is nothing to
/// `Load` (no bytes to read, no row to adopt), so this mints a scratch row
/// exactly as the no-positional launch does and binds it with `bind_new` —
/// the first materialize turns that row into the real file's row through
/// the same no-clobber publish every named draft already uses. Journaling
/// is live from the first keystroke.
///
/// `expect_obs: 0` / `last_known_seq: 0` mirror `open::adopt_scratch_doc`'s
/// own scratch binding: `prepare_materialize` short-circuits to
/// `MaterializePrep::default()` on `bind_new`, so the fabricated `ObsId` is
/// never queried.
///
/// Deliberately does not sweep other sessions' empty scratch rows the way
/// [`bootstrap_untitled_db`] does — a launch this common widens the window
/// in which a concurrent session's freshly minted, not-yet-journaled row
/// could be swept out from under it, trading a harmless leak (the next bare
/// `rune` sweeps it) for possible data loss.
pub(crate) fn bootstrap_new_file(
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
        Err(banner) => {
            return DbBootstrap {
                banner: Some(banner),
                ..DbBootstrap::default()
            };
        }
    };

    let db_id = match blocking_call(&bridge, || store.create_scratch()) {
        Ok(OpOutcome::RowId(db_id)) => db_id,
        Ok(_) => {
            return degrade(store, "internal error: unexpected reply to CreateScratch");
        }
        Err(e) => return degrade(store, format!("create scratch failed: {e}")),
    };

    let db = Db::new(store, bridge, degraded_at_open);
    let doc_db = DocDb::new(db_id, /* bind_new */ true, 0);
    let banner = if degraded_at_open {
        Some(open_warning.unwrap_or_else(|| rune_db::DEGRADED_WARNING.to_string()))
    } else {
        None
    };

    DbBootstrap {
        db: Some(db),
        doc_db: Some(doc_db),
        expect_obs: Some(0),
        recovered_content: None,
        sync_kind: None,
        banner,
    }
}

/// Opens the recovery store with no document to bind (issue #78, the
/// image-first-launch fix): every OTHER bootstrap shape opens the store
/// and then immediately does its own `Load`/scratch-row work against it —
/// this is the shape for a launch whose first positional is an image, which
/// has nothing to `Load` (an image binds no `DocDb` at all) but still needs
/// a live session-wide store so that documents opened LATER in the same
/// session (the Explorer, an extra positional) actually journal. Store-open
/// failure behaves like every other bootstrap shape: reported to stderr,
/// carried on the returned banner, `db: None`.
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
        Err(banner) => {
            return DbBootstrap {
                banner: Some(banner),
                ..DbBootstrap::default()
            };
        }
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
