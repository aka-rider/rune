use std::sync::Arc;
use std::time::Duration;

use rune_vfs::Vfs;

use crate::app::App;
use crate::commands::strip_trailing;
use crate::document::{Document, DocumentId, ReadOnly};
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
    let Some(path) = doc.file_path.clone() else {
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
            crate::runtime::TimerKey::SaveConfirm,
            SAVE_CONFIRM_TIMEOUT,
            Msg::Timer {
                key: crate::runtime::TimerKey::SaveConfirm,
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

#[cfg(test)]
#[path = "save/gate_tests.rs"]
mod gate_tests;
