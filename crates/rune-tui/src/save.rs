//! The start/refusal ladder of the save flow: `trigger_save`'s guards and
//! its plain no-store fallback `Cmd`. The store-backed materialize dance itself
//! (`materialize_now`/`bind_new_now`/`run_materialize_vfs`, the snapshot-
//! autosave debounce) lives in the [`materialize`] submodule; the ack/
//! reaction side — everything from the recovery store's first reply onward
//! — is [`crate::materialize_ack`].
//!
//! `Document::begin_prepare` carries the caller-captured content/path/CAS
//! facts between hops (captured once, at trigger time, never re-derived) as
//! part of the document's own `SaveState::Preparing`/`Publishing`.

use std::sync::Arc;
use std::time::Duration;

use rune_vfs::Vfs;

use crate::app::App;
use crate::commands::strip_trailing;
use crate::document::{DocumentId, ReadOnly};
use crate::materialize_ack;
use crate::messages;
use crate::runtime::{Cmd, CmdError, Effects, Msg};

pub(crate) mod gate;
mod materialize;
use gate::SaveEntry;
use materialize::materialize_now;
pub(crate) use materialize::{bind_new_now, run_materialize_vfs, schedule_snapshot_debounce};

/// The degraded-save confirm-gate's arm-to-confirm window — mirrors
/// `app::CONFIRM_TIMEOUT`: a pending-confirm state like the existing
/// quit-confirm pattern.
const SAVE_CONFIRM_TIMEOUT: Duration = Duration::from_secs(2);

/// Distinguishes an ordinary save's compare-and-swap publish from the
/// disk-conflict Guard's `[S]ave anyway` escape hatch. `Force` is
/// deliberately not a bool: a caller reading `mode == SaveMode::Force` at a
/// call site says what it means, where a bare `true` would not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SaveMode {
    /// The compare-and-swap publish: refuses when the live destination
    /// disagrees with the last baseline this session recorded.
    Normal,
    /// Existence-aware and unconditional: publishes over whatever is
    /// actually at the destination (`exchange` when it exists, `rename_excl`
    /// when it doesn't) and captures whatever that displaces as a durable
    /// blob regardless of any hash comparison — the disk-conflict Guard's
    /// `[S]ave anyway` must never refuse a second time.
    Force,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SaveOrigin {
    Interactive,
    Guard,
}

/// What a `trigger_save` attempt actually did — replaces the old
/// bare `()` return so no refusal is silent to the CALLER, not just to the
/// footer: the quit-save fan-out keys off this to decide whether a
/// document is actually waiting on a save before it counts toward a quit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

/// `super+s` routes it through `rune-db`'s `materialize` on the writer FIFO
/// when a store is present. Guarded by `id`'s in-flight flag (a second
/// `super+s` before the first save's ack reports back is a no-op) and by
/// the re-derived dirty check (nothing to persist otherwise).
///
/// When the store is degraded (open-ladder fallback or a later
/// `on_store_failure`), the FIRST `super+s` only arms a confirm gate
/// tagged with `id`, mirroring `app::handle_quit_key`'s
/// `pending_quit` shape — a document with no durable recovery journal can
/// still be saved, but only once the user has explicitly acknowledged that
/// crash protection is off; a SECOND `super+s` for the SAME document within
/// the window proceeds.
///
/// With no store at all, or with this particular document unbound to one
/// (a document opened via the Explorer lands with
/// `db: None` until per-doc hydration exists), falls back to the direct
/// unconditional-publish `Cmd` ([`save_cmd`]) — Prime Directive: the user
/// must always be able to save; losing the DB never damages a user file.
pub(crate) fn trigger_save(
    app: &mut App,
    id: DocumentId,
    mode: SaveMode,
    origin: SaveOrigin,
    effects: &mut Effects,
) -> SaveStart {
    // Structural, not per-call-site: EVERY save entry point (^S, the
    // DirtyClose/DirtyQuit guards' [S], the quit fan-out, DiskConflict's
    // [S]ave anyway) funnels through this ladder, and the `SaveClearance` it
    // mints is the only key the enqueue/spawn sites below accept — a save
    // that never climbed it cannot be written.
    let clearance = match gate::clear(app, id, SaveEntry::Materialize) {
        Ok(clearance) => clearance,
        Err(start) => return start,
    };
    match origin {
        SaveOrigin::Interactive => {
            if let Some(message) = reading_refusal(app, id) {
                messages::warn(app, message);
                return SaveStart::Refused;
            }
            strip_trailing::strip_trailing_whitespace(app, id);
        }
        SaveOrigin::Guard => strip_trailing::leave_reading_then_strip(app, id),
    }
    // Re-derived, not read from the cache: a transition-quality
    // answer, exactly like the close/quit guards' own `is_dirty_now` calls.
    // `Force` skips this: "save anyway" means "make disk hold my buffer" —
    // the user may have undone back to `saved_content` while disk still
    // holds the foreign bytes the disk-conflict Guard warned about.
    if mode == SaveMode::Normal && !materialize_ack::is_dirty_now(app, id) {
        return SaveStart::NotDirty;
    }
    let Some(doc) = app.doc(id) else {
        return SaveStart::Refused;
    };
    let version = doc.buffer.version();
    let Some(path) = doc.file_path.clone() else {
        // A pathless draft (including the default untitled document a
        // no-arg launch opens) has nothing to save yet — ^S here means
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

    let has_binding = app.db.is_some() && doc.is_store_bound();
    if !has_binding {
        // No store at all, or this document has no binding to it — the
        // direct-vfs fallback. `content` is captured HERE, once,
        // through `Document::begin_save` — the chokepoint that pairs
        // `save_in_flight` with the exact bytes this save will persist, so
        // `handle_save_done`'s eventual ack can only ever promote THESE
        // bytes into `saved_content`, never whatever the buffer holds by
        // the time the ack lands.
        let content: Arc<str> = Arc::from(doc.buffer.content());
        materialize::save_directly(app, id, path, version, content, &clearance, effects);
        return SaveStart::InFlight;
    }

    let degraded = app.db.as_ref().is_some_and(|db| db.degraded);
    if degraded {
        // `Force` is already the user's explicit last-resort consent from
        // the disk-conflict Guard's `[S]ave anyway` — a second confirm
        // dance would silently downgrade that consent instead of acting on
        // it. Any confirm `id` itself had armed is stale the moment this
        // Force save starts, so it is cleared rather than left to fire
        // later against a save that already happened.
        if mode == SaveMode::Force {
            if app.pending_save_confirm.is_some_and(|(cid, _)| cid == id) {
                app.pending_save_confirm = None;
            }
            materialize_now(app, id, path, version, mode, &clearance, effects);
            return SaveStart::InFlight;
        }
        if app.pending_save_confirm.is_some_and(|(cid, _)| cid == id) {
            app.pending_save_confirm = None;
            materialize_now(app, id, path, version, mode, &clearance, effects);
            return SaveStart::InFlight;
        }
        let generation = app.next_save_confirm_gen.mint();
        app.pending_save_confirm = Some((id, generation));
        // `App::pending_save_confirm` is a single global slot, so a caller
        // driving MORE than one document through
        // this arm in succession (the quit-save fan-out) would otherwise
        // overwrite an earlier arm with a later one and leave the status
        // naming nothing — the fan-out is the one caller responsible for not
        // doing that (it stops at the first arm it sees). Naming the
        // document here is what makes the surviving sentence true no matter
        // which caller reaches it.
        let name = app.doc(id).map(crate::title::name_for).unwrap_or_default();
        let save_key = crate::global::label_for(crate::global::GlobalCommand::Save);
        messages::error(
            app,
            format!("recovery disabled for {name} \u{2014} press {save_key} again to save anyway"),
        );
        app.timers.arm(
            crate::runtime::TimerKey::SaveConfirm,
            SAVE_CONFIRM_TIMEOUT,
            Msg::SaveConfirmTimeout { generation },
        );
        return SaveStart::Refused;
    }

    materialize_now(app, id, path, version, mode, &clearance, effects);
    SaveStart::InFlight
}

fn reading_refusal(app: &App, id: DocumentId) -> Option<&'static str> {
    let doc = app.doc(id)?;
    if doc.read_only == ReadOnly::Reading {
        doc.read_only.refusal_message()
    } else {
        None
    }
}

/// The off-thread save I/O itself: an unconditional `rune_vfs::put`
/// (`Force`, no baseline — a durable temp-write + atomic publish) writes
/// EXACTLY `bytes` verbatim — no normalization anywhere on this path.
/// Reached when `id` has no store binding (see `trigger_save`'s docs), or
/// as the fallback when a store binding exists but its `MaterializePrepare`
/// enqueue itself failed (the store couldn't even do the bookkeeping-only
/// first step) — either way, the Prime Directive holds: the user can
/// always save. A publish whose durability confirmation failed is still a
/// success (`durable: false`), surfaced as a warning on the ack side —
/// never a save failure.
fn save_cmd(
    id: DocumentId,
    ticket: crate::document::SaveTicket,
    vfs: std::sync::Arc<dyn Vfs + Send + Sync>,
    path: std::path::PathBuf,
    bytes: Vec<u8>,
    version: u64,
) -> Cmd {
    Cmd::save(move || {
        let (result, durable) = match rune_vfs::put(
            vfs.as_ref(),
            &path,
            &bytes,
            rune_vfs::PutCondition::Force { expect: None },
        ) {
            Ok(
                rune_vfs::PutOutcome::Committed { durable, .. }
                | rune_vfs::PutOutcome::Raced { durable, .. },
            ) => (Ok(()), durable),
            Ok(_) => (
                Err(CmdError::Refused(
                    "save failed: unconditional publish refused".to_string(),
                )),
                true,
            ),
            Err(e) => (Err(CmdError::Io(e)), true),
        };
        Some(Msg::SaveDone {
            id,
            ticket,
            version,
            result,
            durable,
        })
    })
}

// The save/ack/dirty-flow unit tests that used to live here moved to
// `tests/save_flow.rs`: every item they exercise — `App`, `update`, `Msg`,
// `Effects`, `keymap` types, `commands::edit::insert_char` — is already
// public.

#[cfg(test)]
#[path = "save/gate_tests.rs"]
mod gate_tests;
