use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use rune_vfs::{PutCondition, PutOutcome, Vfs};

use crate::app::App;
use crate::commands::strip_trailing;
use crate::document::{Document, DocumentId, ReadOnly};
use crate::materialize_ack::SaveRace;
use crate::messages;
use crate::runtime::{Cmd, CmdError, Effects, Msg};

pub(crate) mod gate;
mod materialize;
use gate::SaveEntry;
use materialize::materialize_now;
pub(crate) use materialize::{bind_new_now, run_materialize_vfs, schedule_snapshot_debounce};

const SAVE_CONFIRM_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SaveMode {
    Normal,
    Force,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SaveOrigin {
    Interactive,
    Guard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SaveStart {
    InFlight,
    NotDirty,
    NeedsName,
    Refused,
}

pub(crate) fn trigger_save(
    app: &mut App,
    id: DocumentId,
    mode: SaveMode,
    origin: SaveOrigin,
    effects: &mut Effects,
) -> SaveStart {
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
    if mode == SaveMode::Normal && !app.doc(id).is_some_and(Document::is_dirty) {
        return SaveStart::NotDirty;
    }
    let Some(doc) = app.doc(id) else {
        return SaveStart::Refused;
    };
    let version = doc.buffer.version();
    let Some(path) = doc.path().map(std::path::Path::to_path_buf) else {
        app.focus_title();
        messages::info(
            app,
            "name this document to save it \u{2014} press Enter when done",
        );
        return SaveStart::NeedsName;
    };

    let has_binding = app.db.is_some() && doc.is_store_bound();
    if !has_binding {
        let content: Arc<str> = Arc::from(doc.buffer.content());
        materialize::save_directly(app, id, path, version, content, &clearance, effects);
        return SaveStart::InFlight;
    }

    let degraded = app.db.as_ref().is_some_and(|db| db.degraded);
    if degraded {
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
        let name = app.doc(id).map(crate::title::name_for).unwrap_or_default();
        let save_key = crate::global::label_for(crate::global::GlobalCommand::Save);
        messages::error(
            app,
            format!("recovery disabled for {name} \u{2014} press {save_key} again to save anyway"),
        );
        app.timers.arm(
            crate::runtime::TimerKey::from(crate::runtime::TimerMsgKey::SaveConfirm),
            SAVE_CONFIRM_TIMEOUT,
            Msg::Timer {
                key: crate::runtime::TimerMsgKey::SaveConfirm,
                generation: generation.raw(),
            },
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

fn save_cmd(
    id: DocumentId,
    ticket: crate::document::SaveTicket,
    vfs: std::sync::Arc<dyn Vfs + Send + Sync>,
    path: std::path::PathBuf,
    bytes: Vec<u8>,
    version: u64,
) -> Cmd {
    Cmd::save(move || {
        let (result, durable, stray_temp, race) = match rune_vfs::put(
            vfs.as_ref(),
            &path,
            &bytes,
            PutCondition::Force { expect: None },
        ) {
            Ok(PutOutcome::Committed {
                durable,
                stray_temp,
                ..
            }) => (Ok(()), durable, stray_temp, None),
            Ok(PutOutcome::Raced {
                durable,
                stray_temp,
                displaced,
                ..
            }) => {
                let race = preserve_displaced(vfs.as_ref(), &path, &displaced.bytes);
                (Ok(()), durable, stray_temp, Some(race))
            }
            Ok(_) => (
                Err(CmdError::Refused(
                    "save failed: unconditional publish refused".to_string(),
                )),
                true,
                None,
                None,
            ),
            Err(e) => (Err(CmdError::Io(e)), true, None, None),
        };
        Some(Msg::SaveDone {
            id,
            ticket,
            version,
            result,
            detail: crate::runtime::SaveOutcomeDetail {
                durable,
                stray_temp,
                race,
            },
        })
    })
}

// A `Force { expect: None }` publish carries no informed baseline, so
// `rune_vfs::put` conservatively flags any pre-existing, differing content
// it displaced as `Raced` rather than silently discarding it. This
// direct-vfs fallback has no recovery store to hand those bytes to, so it
// durably preserves them itself in a fresh, never-clobbered sibling file
// next to `path`. A failure to write that sibling is reported too, never
// swallowed — the primary save already succeeded either way.
fn preserve_displaced(vfs: &dyn Vfs, path: &Path, displaced: &[u8]) -> SaveRace {
    let sibling = conflict_sibling_path(path);
    match rune_vfs::put(vfs, &sibling, displaced, PutCondition::IfAbsent) {
        Ok(PutOutcome::Committed { .. }) => SaveRace::Preserved(sibling),
        Ok(other) => SaveRace::PreserveFailed(format!(
            "could not claim a fresh sibling path at {}: {other:?}",
            sibling.display()
        )),
        Err(e) => SaveRace::PreserveFailed(e.to_string()),
    }
}

fn conflict_sibling_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    path.with_file_name(format!("{file_name}.conflict-{pid}-{nanos}"))
}

#[cfg(test)]
#[path = "save/gate_tests.rs"]
mod gate_tests;
