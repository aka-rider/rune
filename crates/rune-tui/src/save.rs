//! The start/refusal ladder of the save flow (plan WP1.S5 first extracted
//! this out of `app.rs`; plan WP1's dirtiness rework split it again to stay
//! under the 500-line budget): `trigger_save`'s guards and its plain
//! no-store fallback `Cmd`. The store-backed materialize dance itself
//! (`materialize_now`/`bind_new_now`/`run_materialize_vfs`, the snapshot-
//! autosave debounce) lives in the [`materialize`] submodule; the ack/
//! reaction side — everything from the recovery store's first reply onward
//! — is [`crate::materialize_ack`].
//!
//! `App::pending_materialize` carries the caller-captured
//! content/path/CAS facts between hops (captured once, at
//! trigger time, never re-derived).

use std::sync::Arc;
use std::time::Duration;

use rune_syntax::DocumentKind;
use rune_vfs::Vfs;

use crate::app::App;
use crate::document::DocumentId;
use crate::materialize_ack;
use crate::messages;
use crate::runtime::{Cmd, CmdKind, Effects, Msg};

mod materialize;
use materialize::materialize_now;
pub(crate) use materialize::{
    PendingMaterialize, bind_new_now, run_materialize_vfs, schedule_snapshot_debounce,
};

/// The degraded-save confirm-gate's arm-to-confirm window — mirrors
/// `app::CONFIRM_TIMEOUT` (plan WP5.S2/S6: "a pending-confirm state like the
/// existing quit-confirm pattern").
const SAVE_CONFIRM_TIMEOUT: Duration = Duration::from_secs(2);

/// What a `trigger_save` attempt actually did (plan WP1) — replaces the old
/// bare `()` return so no refusal is silent to the CALLER, not just to the
/// footer: WP2's quit-save fan-out keys off this to decide whether a
/// document is actually waiting on a save before it counts toward a quit.
pub(crate) enum SaveStart {
    /// A save is now running (the store-backed materialize dance or the
    /// no-store fallback `Cmd`) — or one already was, before this call.
    InFlight,
    /// Nothing to persist: re-deriving dirty found the buffer already
    /// matches `saved_content`.
    NotDirty,
    /// A pathless draft has nothing to save TO yet — the title field is
    /// now focused so the user can name it.
    NeedsName,
    /// Refused outright: an image document (never overwrite it
    /// with the buffer's own empty bytes), a `Preview` document (transient,
    /// not yet committed to), a rename in flight, or a degraded-store
    /// confirm gate that just armed (or is still pending) — every arm
    /// reaching this sets its own status explaining why.
    Refused,
}

/// `super+s` (WP9, plan Context "Save"; WP5.S6 routes it through
/// `rune-db`'s `materialize` on the writer FIFO when a store is present).
/// Guarded by `id`'s in-flight flag (a second `super+s` before the first
/// save's ack reports back is a no-op) and by the re-derived dirty check
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
pub(crate) fn trigger_save(app: &mut App, id: DocumentId, effects: &mut Effects) -> SaveStart {
    let Some(kind) = app.doc(id).map(|d| d.kind) else {
        return SaveStart::Refused;
    };
    // Plan WP4.S9: an image document has a REAL
    // `file_path`, so without this a save would reach `save_cmd` and
    // overwrite it with the buffer's own (always empty) bytes. Placed
    // FIRST, before the in-flight/dirty checks below — those already
    // return early for an unedited buffer, which would make a guard placed
    // after them dead code.
    if kind == DocumentKind::Image {
        return SaveStart::Refused;
    }
    // Every global save chord routes here unconditionally,
    // and the no-store fallback below reaches `vfs.save_atomic` directly —
    // without this, saving a `Preview` document would atomically overwrite
    // the previewed file with this document's own buffer.
    if app.refuse_if_preview(id) {
        return SaveStart::Refused;
    }
    if app.doc(id).is_some_and(|d| d.save_in_flight) {
        messages::warn(app, "a save is already in progress");
        return SaveStart::InFlight;
    }
    // The mirror of `rename::begin`'s own `save_in_flight` refusal, and
    // required for the same reason from the other side: a save `Cmd`
    // captures the document's path when it is spawned, while the rebind to
    // the renamed path only happens once the rename ack lands. Saving in
    // between republishes the edited content at the OLD path — resurrecting
    // the file the rename is in the middle of moving away from, and leaving
    // the new name holding stale bytes. Refused rather than queued: the ack
    // is one message away, and ⌘S again after it lands does the right thing
    // against the right path.
    if app.rename.in_flight() {
        // `error`, not `warn`: nothing was written, and unlike the
        // in-progress-save refusal above (where the data genuinely is
        // being written) or merge mode's own refusal (where the footer's
        // merge hint keeps that state visible independently), an
        // auto-collapsing message here would leave this refusal with no
        // trace once its 5s window elapses.
        messages::error(app, "can't save while a rename is in flight");
        return SaveStart::Refused;
    }
    // Re-derived, not read from the cache (plan WP1): a transition-quality
    // answer, exactly like the close/quit guards' own `is_dirty_now` calls.
    if !materialize_ack::is_dirty_now(app, id) {
        return SaveStart::NotDirty;
    }
    let Some(doc) = app.doc(id) else {
        return SaveStart::Refused;
    };
    let version = doc.buffer.version();
    let Some(path) = doc.file_path.clone() else {
        // A pathless draft (including the default untitled document a
        // no-arg launch opens) has nothing to save yet — ⌘S here means
        // "name it", so route it into the same "pathless draft is a
        // CREATE" flow `rename::begin` already implements
        // (`rename.rs` -> `bind_new`): focus the title field so the
        // user can type a name; Enter from there commits the create, and
        // `Document::bind_path` (routed through by both `bind_to` and
        // `handle_materialize_ack` below) is what actually switches the
        // title off the placeholder once the file exists.
        app.focus_title();
        messages::info(
            app,
            "name this document to save it \u{2014} press Enter when done",
        );
        return SaveStart::NeedsName;
    };

    let has_binding = app.db.is_some() && doc.db.is_some();
    if !has_binding {
        // No store at all, or this document has no binding to it — the
        // pre-WP5 direct-vfs fallback. `content` is captured HERE, once,
        // through `Document::begin_save` — the chokepoint that pairs
        // `save_in_flight` with the exact bytes this save will persist, so
        // `handle_save_done`'s eventual ack can only ever promote THESE
        // bytes into `saved_content`, never whatever the buffer holds by
        // the time the ack lands.
        let content: Arc<str> = Arc::from(doc.buffer.content());
        let bytes = content.as_bytes().to_vec();
        if let Some(doc) = app.doc_mut(id) {
            doc.begin_save(version, content);
        }
        let vfs = Arc::clone(&app.vfs);
        effects.cmds.push(save_cmd(id, vfs, path, bytes, version));
        return SaveStart::InFlight;
    }

    let degraded = app.db.as_ref().is_some_and(|db| db.degraded);
    if degraded {
        if app.pending_save_confirm.is_some_and(|(cid, _)| cid == id) {
            app.pending_save_confirm = None;
            materialize_now(app, id, path, version, effects);
            return SaveStart::InFlight;
        }
        let generation = app.next_save_confirm_gen;
        app.next_save_confirm_gen = app.next_save_confirm_gen.wrapping_add(1);
        app.pending_save_confirm = Some((id, generation));
        // `App::pending_save_confirm` is a single global slot (plan WP1
        // decision 3), so a caller driving MORE than one document through
        // this arm in succession (the quit-save fan-out) would otherwise
        // overwrite an earlier arm with a later one and leave the status
        // naming nothing — the fan-out is the one caller responsible for not
        // doing that (it stops at the first arm it sees). Naming the
        // document here is what makes the surviving sentence true no matter
        // which caller reaches it.
        let name = app.doc(id).map(crate::title::name_for).unwrap_or_default();
        messages::error(
            app,
            format!("recovery disabled for {name} \u{2014} press \u{2318}S again to save anyway"),
        );
        effects.cmds.push(save_confirm_timeout_cmd(generation));
        return SaveStart::Refused;
    }

    materialize_now(app, id, path, version, effects);
    SaveStart::InFlight
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

/// The off-thread save I/O itself: `vfs.save_atomic` (a durable
/// temp-write + atomic publish, or `Mem`'s test double) writes EXACTLY
/// `bytes` verbatim — no normalization anywhere on this path.
/// Reached when `id` has no store binding (see `trigger_save`'s docs), or
/// as WP7's fallback when a store binding exists but its `MaterializePrepare`
/// enqueue itself failed (the store couldn't even do the bookkeeping-only
/// first step) — either way, the Prime Directive holds: the user can
/// always save.
pub(crate) fn save_cmd(
    id: DocumentId,
    vfs: std::sync::Arc<dyn Vfs + Send + Sync>,
    path: std::path::PathBuf,
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

// The save/ack/dirty-flow unit tests that used to live here moved to
// `tests/save_flow.rs` (plan WP1.S5, same rationale as `app.rs`'s
// extraction: every item they exercise — `App`, `update`, `Msg`,
// `Effects`, `keymap` types, `commands::edit::insert_char` — is already
// public).
