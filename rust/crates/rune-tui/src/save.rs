//! The save/ack/dirty flow (plan WP1.S5, extracted out of `app.rs` to keep
//! it under the §1.6 line budget): `trigger_save`'s degraded-store confirm
//! gate, `materialize_now`'s CAS-enqueue, the `Msg::SaveDone`/materialize-ack
//! reactions, the dirty-cache recompute chokepoint (§1.4.8), the snapshot-
//! autosave debounce, and `on_store_failure`'s whole-store degrade. Every
//! function here is per-document except `on_store_failure`, which stays
//! app-wide (plan decision 3/6: a hard write failure degrades the ONE
//! shared `Store`, never just the document that happened to trigger it).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use rune_db::MatResult;
use rune_vfs::Vfs;

use crate::app::{App, StatusSource};
use crate::document::DocumentId;
use crate::runtime::{Cmd, CmdKind, Effects, Msg};

/// The degraded-save confirm-gate's arm-to-confirm window — mirrors
/// `app::CONFIRM_TIMEOUT` (plan WP5.S2/S6: "a pending-confirm state like the
/// existing quit-confirm pattern").
const SAVE_CONFIRM_TIMEOUT: Duration = Duration::from_secs(2);

/// The snapshot-autosave debounce window (plan WP5.S6, port of
/// `workspace_timers.go:11`'s 2s debounce).
const SNAPSHOT_DEBOUNCE: Duration = Duration::from_secs(2);

/// `super+s` (WP9, plan Context "Save"; WP5.S6 routes it through
/// `rune-db`'s `materialize` on the writer FIFO when a store is present).
/// Guarded by `id`'s in-flight flag (a second `super+s` before the first
/// save's ack reports back is a no-op) and by `version != saved_version`
/// (nothing to persist otherwise).
///
/// When the store is degraded (open-ladder fallback or a later
/// `on_store_failure`), the FIRST `super+s` only arms a confirm gate
/// tagged with `id` (plan WP1 decision 3, mirrors `app::handle_quit_key`'s
/// `pending_quit` shape) — a document with no durable recovery journal can
/// still be saved, but only once the user has explicitly acknowledged that
/// crash protection is off; a SECOND `super+s` for the SAME document within
/// the window proceeds.
///
/// With no store at all, or with this particular document unbound to one
/// (Assumption A1: a document opened after WP4's Explorer lands with
/// `db: None` until per-doc hydration exists), falls back to the pre-WP5
/// direct `vfs.save_atomic` `Cmd` — Prime Directive: the user must always be
/// able to save (plan decision 5: "losing the DB never damages a user
/// file").
pub(crate) fn trigger_save(app: &mut App, id: DocumentId, effects: &mut Effects) {
    if app.doc(id).save_in_flight {
        return;
    }
    let version = app.doc(id).buffer.version();
    if version == app.doc(id).saved_version {
        return;
    }
    let Some(path) = app.doc(id).file_path.clone() else {
        app.set_status(
            "no file to save \u{2014} rune was opened without a path",
            StatusSource::SaveError,
        );
        return;
    };

    let has_binding = app.db.is_some() && app.doc(id).db.is_some();
    if !has_binding {
        // No store at all, or this document has no binding to it — the
        // pre-WP5 direct-vfs fallback.
        app.doc_mut(id).save_in_flight = true;
        let bytes = app.doc(id).buffer.content().as_bytes().to_vec();
        let vfs = Arc::clone(&app.vfs);
        effects.cmds.push(save_cmd(id, vfs, path, bytes, version));
        return;
    }

    let degraded = app.db.as_ref().is_some_and(|db| db.degraded);
    if degraded {
        if app.pending_save_confirm.is_some_and(|(cid, _)| cid == id) {
            app.pending_save_confirm = None;
            materialize_now(app, id, path, version);
        } else {
            let generation = app.next_save_confirm_gen;
            app.next_save_confirm_gen = app.next_save_confirm_gen.wrapping_add(1);
            app.pending_save_confirm = Some((id, generation));
            app.set_status(
                "recovery disabled \u{2014} press \u{2318}S again to save anyway",
                StatusSource::Other,
            );
            effects.cmds.push(save_confirm_timeout_cmd(generation));
        }
        return;
    }

    materialize_now(app, id, path, version);
}

/// Enqueues `content` to `rune-db`'s writer FIFO via `Store::materialize`
/// (plan WP5.S6). Not a `Cmd`: `Store::enqueue` is a plain, non-blocking
/// channel send (never I/O that leaves this thread — the actual disk write
/// happens on the writer thread, whose eventual `DbEvent::Ok{ result:
/// OpOutcome::Materialize(..), .. }` ack arrives as `Msg::Db`, routed back
/// to `id` via `app.db_ops`, handled by `handle_materialize_ack`), so §5.4
/// lets `update` call it directly. `content`/`expect`/`seq`/`bind_new` are
/// all captured HERE, synchronously — never re-derived once the op runs
/// (§1.4.2/§1.4.8).
fn materialize_now(app: &mut App, id: DocumentId, path: PathBuf, version: u64) {
    let Some((db_id, expect_obs, last_known_seq, bind_new)) = app
        .doc(id)
        .db
        .as_ref()
        .map(|d| (d.db_id, d.expect_obs, d.last_known_seq, d.bind_new))
    else {
        return;
    };
    let content = app.doc(id).buffer.content().to_string();
    let Some(db) = app.db.as_ref() else { return };
    let result = db
        .store
        .materialize(db_id, &path, &content, expect_obs, last_known_seq, bind_new);

    app.doc_mut(id).save_in_flight = true;
    app.doc_mut(id).save_pending_version = Some(version);
    match result {
        Ok(op_id) => {
            app.db_ops.insert(op_id, id);
        }
        Err(e) => {
            // A store enqueue-time failure is exactly the same class of
            // event as an async `DbEvent::Err`/`Fatal` (plan decision 3) —
            // degrade the store and raise the sticky banner via the same
            // chokepoint `db::append_edit`/`move_undo_pos` use, not a
            // one-shot `SaveError` status that leaves the store untouched
            // and lets the next save silently retry against an already-
            // wedged writer.
            on_store_failure(app, e.to_string());
        }
    }
}

/// The reaction to a `materialize` ack for `id` (plan WP5.S6): advances
/// `saved_version`/`DocDb::expect_obs`/`bind_new` on a commit, surfaces each
/// `MatResult` outcome as status text, and — either way — clears `id`'s
/// `save_in_flight` and recomputes its dirty cache (trigger (b) of
/// `recompute_dirty`'s doc comment).
pub(crate) fn handle_materialize_ack(app: &mut App, id: DocumentId, mat: MatResult) {
    app.doc_mut(id).save_in_flight = false;
    let pending_version = app.doc_mut(id).save_pending_version.take();

    if mat.committed {
        if let Some(saved) = &mat.saved
            && let Some(doc_db) = app.doc_mut(id).db.as_mut()
        {
            doc_db.expect_obs = saved.id;
            doc_db.bind_new = false;
        }
        if let Some(version) = pending_version
            && version > app.doc(id).saved_version
        {
            app.doc_mut(id).saved_version = version;
        }
        if app.status_source == StatusSource::SaveError {
            app.status_message = None;
        }
        if mat.raced {
            app.set_status(
                "saved \u{2014} a concurrent external change was overwritten; its bytes were preserved",
                StatusSource::Other,
            );
        }
    } else if mat.missing {
        app.set_status(
            "save failed: file no longer exists",
            StatusSource::SaveError,
        );
    } else {
        app.set_status(
            "save refused \u{2014} the file changed on disk since it was opened",
            StatusSource::SaveError,
        );
    }
    recompute_dirty(app, id);
}

/// The reaction to `Msg::SaveDone` — the no-store fallback save path's own
/// completion (plan decision 5), or a leftover reply for a document whose
/// store binding vanished mid-flight. Same provenance-aware clear as
/// `handle_materialize_ack` (review finding F2).
pub(crate) fn handle_save_done(
    app: &mut App,
    id: DocumentId,
    version: u64,
    result: Result<(), String>,
) {
    app.doc_mut(id).save_in_flight = false;
    match result {
        Ok(()) => {
            if version > app.doc(id).saved_version {
                app.doc_mut(id).saved_version = version;
            }
            // Provenance-aware clear (review finding F2): only a message
            // THIS save path set (a prior failed/un-attempted save) is
            // dismissed here. An unrelated message — a pbpaste failure, an
            // edit/undo/redo failure — survives a successful save exactly
            // as it already survives a successful edit.
            if app.status_source == StatusSource::SaveError {
                app.status_message = None;
            }
        }
        Err(e) => {
            app.set_status(format!("save failed: {e}"), StatusSource::SaveError);
        }
    }
    recompute_dirty(app, id);
}

/// A stale `generation` (a later journal mutation already rescheduled the
/// debounce — `schedule_snapshot_debounce`) is ignored. `content` and the
/// journal position ("current position", plan WP5.S6) are captured
/// SYNCHRONOUSLY here, in `update` — never re-derived once the enqueued
/// `CreateSnapshot` op actually runs on the writer thread (§1.4.2/§1.4.8's
/// "caller-captured, never re-derived" discipline, same as `materialize`).
/// Never touches the user's file — `create_snapshot` is a pure recovery
/// anchor (`rune-db::snapshot`'s doc comment).
pub(crate) fn handle_snapshot_due(app: &mut App, id: DocumentId, generation: u32) {
    if app.db.as_ref().is_none_or(|db| db.degraded) {
        return;
    }
    let Some((db_id, last_known_seq)) = app
        .doc(id)
        .db
        .as_ref()
        .filter(|d| d.snapshot_generation == generation)
        .map(|d| (d.db_id, d.last_known_seq))
    else {
        return;
    };
    let content = app.doc(id).buffer.content().to_string();
    let Some(db) = app.db.as_ref() else { return };
    let result = db.store.create_snapshot(db_id, &content, last_known_seq);
    match result {
        Ok(op_id) => {
            app.db_ops.insert(op_id, id);
        }
        Err(e) => on_store_failure(app, e.to_string()),
    }
}

/// Bumps `id`'s snapshot-autosave generation and (re)schedules its 2s
/// debounce timer (plan WP5.S6, port of `workspace_timers.go:11`) — called
/// once per message batch that mutated the ACTIVE document's journal, from
/// `app::update`'s wrapper.
pub(crate) fn schedule_snapshot_debounce(app: &mut App, id: DocumentId, effects: &mut Effects) {
    if app.db.is_none() {
        return;
    }
    let Some(doc_db) = app.doc_mut(id).db.as_mut() else {
        return;
    };
    doc_db.snapshot_generation = doc_db.snapshot_generation.wrapping_add(1);
    let generation = doc_db.snapshot_generation;
    effects.cmds.push(snapshot_timeout_cmd(id, generation));
}

fn snapshot_timeout_cmd(id: DocumentId, generation: u32) -> Cmd {
    Cmd::new(CmdKind::SnapshotDebounce, move || {
        std::thread::sleep(SNAPSHOT_DEBOUNCE);
        Some(Msg::SnapshotDue { id, generation })
    })
}

/// A store enqueue-time error or an async `DbEvent::Err`/`Fatal` landed
/// (plan decision 3): the in-memory buffer/journal are NEVER rolled back —
/// only the WHOLE store is marked degraded (sticky; no reopen path) and a
/// persistent banner is raised. If ANY document had a save in flight, its
/// guard is released and the failure surfaces as an ordinary save error
/// too, so `trigger_save`'s in-flight guard can never wedge open on a lost
/// ack — app-wide because one shared `Store`'s failure can strand any
/// document currently mid-save on it, not just the one whose op happened
/// to trigger this call.
pub(crate) fn on_store_failure(app: &mut App, error: String) {
    if let Some(db) = app.db.as_mut() {
        db.degraded = true;
    }
    app.db_banner = Some(format!("recovery disabled: {error}"));

    let mut any_in_flight = false;
    for doc in app.documents.values_mut() {
        if doc.save_in_flight {
            doc.save_in_flight = false;
            doc.save_pending_version = None;
            any_in_flight = true;
        }
    }
    if any_in_flight {
        app.set_status(format!("save failed: {error}"), StatusSource::SaveError);
    }
}

/// CONSTITUTION §1.4.8: `Document::is_dirty` reads only the cache this
/// recomputes — called at exactly two trigger points: (a) any journal
/// mutation (`commands::edit::commit_edit_batch`/`undo`/`redo`, immediately
/// after they mutate `doc.journal`) and (b) any `DbEvent` ack that moves the
/// baseline (`handle_materialize_ack`, immediately after `saved_version`
/// itself changes). The comparison is the buffer-version proxy already
/// established pre-WP5 (`saved_version`, now advanced ONLY by a successful
/// materialize ack or the no-store fallback's `SaveDone`) — both trigger
/// points call this immediately after whichever of `saved_version`/
/// `buffer.version()` they just changed, so the cache never observes a
/// stale pairing.
pub(crate) fn recompute_dirty(app: &mut App, id: DocumentId) {
    let dirty = app.doc(id).buffer.version() != app.doc(id).saved_version;
    app.doc_mut(id).is_dirty_cached = dirty;
}

/// The 2s degraded-save confirm-gate timer (plan WP5.S2/S6) — mirrors
/// `app::quit_confirm_timeout_cmd`'s shape exactly. Doc-agnostic (plan WP1
/// decision 3): the doc tag lives in `App::pending_save_confirm`'s `Option`
/// tuple itself, not in this `Msg`.
fn save_confirm_timeout_cmd(generation: u32) -> Cmd {
    Cmd::new(CmdKind::SaveConfirmTimeout, move || {
        std::thread::sleep(SAVE_CONFIRM_TIMEOUT);
        Some(Msg::SaveConfirmTimeout { generation })
    })
}

/// The off-thread save I/O itself: `vfs.save_atomic` (§1.4.1's durable
/// temp-write + atomic publish, or `Mem`'s test double) writes EXACTLY
/// `bytes` — §1.4.5 byte-verbatim, no normalization anywhere on this path.
/// Only reached when `id` has no store binding — see `trigger_save`'s docs.
fn save_cmd(
    id: DocumentId,
    vfs: Arc<dyn Vfs + Send + Sync>,
    path: PathBuf,
    bytes: Vec<u8>,
    version: u64,
) -> Cmd {
    Cmd::new(CmdKind::Save, move || {
        let result = vfs.save_atomic(&path, &bytes).map_err(|e| e.to_string());
        Some(Msg::SaveDone {
            id,
            version,
            result,
        })
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::app::update;
    use crate::runtime::Msg;
    use rune_core::buffer::Buffer;
    use rune_vfs::{Disk, Mem, Vfs};

    fn test_app() -> App {
        App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None)
    }

    #[test]
    fn save_done_ok_advances_saved_version_and_clears_a_prior_save_failure() {
        let mut app = test_app();
        let id = app.active;
        let version = app.doc(id).buffer.version();

        // A real prior save failure — the only kind of message the
        // provenance-aware clear below (review finding F2) is allowed to
        // dismiss.
        let mut effects = Effects::default();
        update(
            &mut app,
            Msg::SaveDone {
                id,
                version,
                result: Err("oops".to_string()),
            },
            &mut effects,
        );
        assert!(app.status_message.is_some());

        let mut effects2 = Effects::default();
        update(
            &mut app,
            Msg::SaveDone {
                id,
                version,
                result: Ok(()),
            },
            &mut effects2,
        );
        assert_eq!(app.doc(id).saved_version, version);
        assert!(
            app.status_message.is_none(),
            "a successful save must clear the failure message ITS OWN save path set"
        );
    }

    /// Regression for F2: a successful save must not clear a status message
    /// some OTHER subsystem set — e.g. an unresolved `Msg::Error` such as a
    /// pbpaste failure the user hasn't dismissed yet.
    #[test]
    fn save_done_ok_does_not_clear_an_unrelated_status_message() {
        let mut app = test_app();
        let id = app.active;
        let mut effects = Effects::default();
        update(
            &mut app,
            Msg::Error("pbpaste failed to run: No such file or directory".to_string()),
            &mut effects,
        );
        assert!(app.status_message.is_some());

        let version = app.doc(id).buffer.version();
        let mut effects2 = Effects::default();
        update(
            &mut app,
            Msg::SaveDone {
                id,
                version,
                result: Ok(()),
            },
            &mut effects2,
        );

        assert_eq!(app.doc(id).saved_version, version);
        assert!(
            app.status_message.is_some(),
            "a successful save must not clear an unrelated (non-save) status message"
        );
        assert!(
            app.status_message
                .as_deref()
                .is_some_and(|s| s.contains("pbpaste"))
        );
    }

    #[test]
    fn save_done_err_surfaces_status_and_keeps_dirty() {
        let mut app = test_app();
        let id = app.active;
        app.doc_mut(id).buffer = app.doc(id).buffer.insert(0, "x");
        let before_saved = app.doc(id).saved_version;
        let version = app.doc(id).buffer.version();
        let mut effects = Effects::default();
        update(
            &mut app,
            Msg::SaveDone {
                id,
                version,
                result: Err("disk full".to_string()),
            },
            &mut effects,
        );
        assert_eq!(app.doc(id).saved_version, before_saved);
        assert!(app.is_dirty());
        assert!(
            app.status_message
                .as_deref()
                .is_some_and(|s| s.contains("disk full"))
        );
    }

    fn save_key() -> crate::keymap::KeyInput {
        crate::keymap::KeyInput {
            code: crate::keymap::KeyCode::Char('s'),
            mods: crate::keymap::Mods {
                sup: true,
                ..crate::keymap::Mods::NONE
            },
        }
    }

    fn press_save(app: &mut App) -> Effects {
        let mut effects = Effects::default();
        update(app, Msg::Key(save_key()), &mut effects);
        effects
    }

    fn settle_cmds(app: &mut App, effects: Effects) {
        for cmd in effects.cmds {
            if let Some(msg) = cmd.run() {
                let mut next = Effects::default();
                update(app, msg, &mut next);
                settle_cmds(app, next);
            }
        }
    }

    #[test]
    fn save_persists_exact_bytes_for_crlf_bom_and_no_trailing_newline_fixtures() {
        for content in ["a\r\nb\r\n", "\u{feff}hello", "no trailing newline"] {
            let vfs = Arc::new(Mem::new());
            let path = PathBuf::from("/doc.md");
            let mut app = App::new(
                Buffer::new(content),
                Some(path.clone()),
                Arc::clone(&vfs) as Arc<dyn Vfs + Send + Sync>,
                None,
            );
            let id = app.active;
            app.doc_mut(id).saved_version = 0;

            let effects = press_save(&mut app);
            assert_eq!(effects.cmds.len(), 1, "one save Cmd must be spawned");
            settle_cmds(&mut app, effects);

            let saved = vfs.read(&path).expect("save must have written the file");
            assert_eq!(
                saved,
                content.as_bytes(),
                "saved bytes must be byte-identical to the buffer, verbatim"
            );
            assert!(!app.is_dirty());
        }
    }

    #[test]
    fn save_failure_surfaces_a_status_error_and_keeps_dirty() {
        let vfs = Arc::new(Mem::new());
        vfs.fail_next_save(std::io::ErrorKind::Other);
        let path = PathBuf::from("/doc.md");
        let mut app = App::new(
            Buffer::new("hello"),
            Some(path),
            Arc::clone(&vfs) as Arc<dyn Vfs + Send + Sync>,
            None,
        );
        let id = app.active;
        app.doc_mut(id).saved_version = 0;

        let effects = press_save(&mut app);
        settle_cmds(&mut app, effects);

        assert!(app.is_dirty());
        assert!(
            app.status_message.is_some(),
            "a failed save must surface a status-line error"
        );
    }

    #[test]
    fn a_second_save_press_while_one_is_in_flight_is_a_no_op() {
        let mut app = App::new(
            Buffer::new("hello"),
            Some(PathBuf::from("/doc.md")),
            Arc::new(Mem::new()),
            None,
        );
        let id = app.active;
        app.doc_mut(id).buffer = app.doc(id).buffer.insert(0, "x"); // makes it dirty

        let effects = press_save(&mut app);
        assert_eq!(effects.cmds.len(), 1);
        assert!(app.doc(id).save_in_flight);

        let effects2 = press_save(&mut app);
        assert!(
            effects2.cmds.is_empty(),
            "a save already in flight must not spawn a second save Cmd"
        );
        assert!(app.doc(id).save_in_flight);
    }

    #[test]
    fn an_edit_during_a_save_keeps_the_buffer_dirty_once_the_save_completes() {
        let vfs = Arc::new(Mem::new());
        let path = PathBuf::from("/doc.md");
        let mut app = App::new(
            Buffer::new("hello"),
            Some(path),
            Arc::clone(&vfs) as Arc<dyn Vfs + Send + Sync>,
            None,
        );
        let id = app.active;
        app.doc_mut(id).saved_version = 0;

        let effects = press_save(&mut app); // captures the pre-edit version
        assert_eq!(effects.cmds.len(), 1);

        crate::commands::edit::insert_char(&mut app, id, '!');
        let after_edit_version = app.doc(id).buffer.version();

        settle_cmds(&mut app, effects); // delivers SaveDone for the OLD version

        assert!(
            app.doc(id).saved_version < after_edit_version,
            "SaveDone must only advance saved_version to the version IT saved, \
             not the buffer's current (post-edit) version"
        );
        assert!(
            app.is_dirty(),
            "an edit made during the in-flight save must leave the buffer dirty \
             once that save completes"
        );
    }

    #[test]
    fn saving_a_path_that_does_not_exist_on_disk_creates_it_via_the_excl_path() {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("rune-wp9-excl-{}-{n}.md", std::process::id()));
        let _ = std::fs::remove_file(&path); // in case a prior run left it behind
        assert!(!path.exists(), "the fixture path must not exist yet");

        let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Disk);
        let mut app = App::new(
            Buffer::new("brand new file\n"),
            Some(path.clone()),
            vfs,
            None,
        );
        let id = app.active;
        app.doc_mut(id).saved_version = 0;

        let effects = press_save(&mut app);
        settle_cmds(&mut app, effects);

        assert!(!app.is_dirty());
        let saved = std::fs::read(&path).expect("save must have created the file on disk");
        assert_eq!(saved, b"brand new file\n");

        let _ = std::fs::remove_file(&path);
    }
}
