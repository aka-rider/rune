use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::app::{App, QuitIntent};
use crate::document::DocumentId;
use crate::keymap::{KeyCode, KeyInput};
use crate::merge::{MergeIntent, MergeState};
use crate::messages;
use crate::runtime::Effects;
use crate::save::{self, SaveMode, SaveOrigin, SaveStart};
use crate::workspace;

#[must_use]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardRaise {
    Raised,
    Displaced,
}

pub fn set_guard(app: &mut App, prompt: GuardPrompt, effects: &mut Effects) -> GuardRaise {
    if app.guard.is_some() {
        return GuardRaise::Displaced;
    }
    app.close_focus_overlays(effects);
    app.guard = Some(prompt);
    GuardRaise::Raised
}

pub fn set_guard_or_warn(
    app: &mut App,
    prompt: GuardPrompt,
    refused: &str,
    effects: &mut Effects,
) -> GuardRaise {
    let raise = set_guard(app, prompt, effects);
    if raise == GuardRaise::Displaced {
        messages::warn(app, refused);
    }
    raise
}

/// The sole writer of `app.guard = None` — every dismissal path routes here.
pub fn clear_guard(app: &mut App) {
    let was_collision = matches!(
        &app.guard,
        Some(GuardPrompt {
            kind: GuardKind::RenameCollision { .. },
            ..
        })
    );
    app.guard = None;
    if was_collision {
        crate::rename::on_prompt_dismissed(app);
    }
}

pub(crate) fn retract_disk_conflict_on_convergence(
    app: &mut App,
    doc: DocumentId,
    kind: rune_db::SyncKind,
) {
    if kind.is_disk_divergent() {
        return;
    }
    let raised_for_doc = matches!(
        &app.guard,
        Some(GuardPrompt {
            doc: d,
            kind: GuardKind::DiskConflict,
        }) if *d == doc
    );
    if !raised_for_doc {
        return;
    }
    clear_guard(app);
    messages::info(app, "disk settled — save again when ready");
}

pub struct GuardPrompt {
    pub doc: DocumentId,
    pub kind: GuardKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GuardKind {
    DirtyClose,
    DirtyQuit,
    RenameCollision {
        target: String,
    },
    DiskConflict,
    Trash {
        path: PathBuf,
        subject: TrashSubject,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrashSubject {
    File,
    Symlink,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum GuardKey {
    Char(char),
    Escape,
}

impl GuardKey {
    pub fn label(self) -> String {
        match self {
            GuardKey::Char(c) => c.to_ascii_uppercase().to_string(),
            GuardKey::Escape => "\u{238b}".to_string(),
        }
    }
}

pub struct GuardOption {
    pub key: GuardKey,
    pub help: &'static str,
}

impl GuardOption {
    fn answers(&self, key: KeyInput) -> bool {
        match (self.key, key.code) {
            (GuardKey::Char(want), KeyCode::Char(got)) => got.eq_ignore_ascii_case(&want),
            (GuardKey::Escape, KeyCode::Escape) => true,
            _ => false,
        }
    }
}

pub const DIRTY_CLOSE_SAVE: GuardOption = GuardOption {
    key: GuardKey::Char('s'),
    help: "save",
};
pub const DIRTY_CLOSE_DISCARD: GuardOption = GuardOption {
    key: GuardKey::Char('d'),
    help: "discard",
};
pub const DIRTY_CLOSE_OPTIONS: &[GuardOption] = &[DIRTY_CLOSE_SAVE, DIRTY_CLOSE_DISCARD];

pub const GUARD_CANCEL: GuardOption = GuardOption {
    key: GuardKey::Escape,
    help: "cancel",
};

pub const RENAME_REPLACE: GuardOption = GuardOption {
    key: GuardKey::Char('r'),
    help: "replace",
};
pub const RENAME_COLLISION_OPTIONS: &[GuardOption] = &[RENAME_REPLACE];

pub const DISK_CONFLICT_SAVE: GuardOption = GuardOption {
    key: GuardKey::Char('s'),
    help: "save anyway",
};
pub const DISK_CONFLICT_DISCARD: GuardOption = GuardOption {
    key: GuardKey::Char('d'),
    help: "discard",
};
pub const DISK_CONFLICT_MERGE: GuardOption = GuardOption {
    key: GuardKey::Char('m'),
    help: "merge",
};
pub const DISK_CONFLICT_OPTIONS: &[GuardOption] = &[
    DISK_CONFLICT_SAVE,
    DISK_CONFLICT_DISCARD,
    DISK_CONFLICT_MERGE,
];

pub const TRASH_YES: GuardOption = GuardOption {
    key: GuardKey::Char('y'),
    help: "yes",
};
pub const TRASH_OPTIONS: &[GuardOption] = &[TRASH_YES];

fn cancel_status(kind: &GuardKind) -> &'static str {
    match kind {
        GuardKind::DirtyClose => "close cancelled",
        GuardKind::DirtyQuit => "quit cancelled",
        GuardKind::RenameCollision { .. } => "rename cancelled",
        GuardKind::DiskConflict => "save cancelled",
        GuardKind::Trash { .. } => "trash cancelled",
    }
}

pub(crate) fn handle_guard_key(app: &mut App, key: KeyInput, effects: &mut Effects) {
    let Some(prompt) = &app.guard else {
        return;
    };
    let doc = prompt.doc;
    if GUARD_CANCEL.answers(key) {
        let msg = cancel_status(&prompt.kind);
        clear_guard(app);
        messages::info(app, msg);
        return;
    }
    match &prompt.kind {
        GuardKind::DirtyClose => handle_dirty_close_key(app, doc, key, effects),
        GuardKind::DirtyQuit => handle_dirty_quit_key(app, key, effects),
        GuardKind::RenameCollision { .. } => handle_rename_collision_key(app, key),
        GuardKind::DiskConflict => {
            handle_disk_conflict_key(app, doc, key, effects);
        }
        GuardKind::Trash { path, .. } => handle_trash_key(app, path.clone(), key, effects),
    }
}

fn handle_dirty_close_key(app: &mut App, doc: DocumentId, key: KeyInput, effects: &mut Effects) {
    if DIRTY_CLOSE_SAVE.answers(key) {
        clear_guard(app);
        let already_in_flight = app
            .doc(doc)
            .is_some_and(super::document::Document::save_in_flight);
        let start = save::trigger_save(app, doc, SaveMode::Normal, SaveOrigin::Guard, effects);
        if already_in_flight {
            messages::warn(
                app,
                "close cancelled \u{2014} a save was already in progress; try again once it \
                 finishes",
            );
        } else if matches!(start, SaveStart::InFlight) {
            app.pending_close_on_save = Some(doc);
        }
    } else if DIRTY_CLOSE_DISCARD.answers(key) {
        clear_guard(app);
        let _ = crate::workspace::close_now(app, doc, effects);
    }
}

fn handle_dirty_quit_key(app: &mut App, key: KeyInput, effects: &mut Effects) {
    if DIRTY_CLOSE_DISCARD.answers(key) {
        clear_guard(app);
        app.should_quit = true;
    } else if DIRTY_CLOSE_SAVE.answers(key) {
        clear_guard(app);
        start_quit_save_fan_out(app, effects);
    }
}

fn start_quit_save_fan_out(app: &mut App, effects: &mut Effects) {
    let docs = crate::pane::unpreserved_dirty_docs(app);
    let mut pending = BTreeMap::new();
    for id in docs {
        let already_in_flight = app
            .doc(id)
            .is_some_and(super::document::Document::save_in_flight);
        match save::trigger_save(app, id, SaveMode::Normal, SaveOrigin::Guard, effects) {
            SaveStart::InFlight if already_in_flight => {
                messages::warn(
                    app,
                    "quit cancelled \u{2014} a save was already in progress; try again once it \
                     finishes",
                );
                app.quit = crate::app::QuitNegotiation::Idle;
                return;
            }
            SaveStart::InFlight => {
                if let Some(version) = app
                    .doc(id)
                    .and_then(super::document::Document::pending_save_version)
                {
                    pending.insert(id, version);
                }
            }
            SaveStart::Refused if app.pending_save_confirm.is_some_and(|(cid, _)| cid == id) => {
                break;
            }
            SaveStart::NotDirty | SaveStart::NeedsName | SaveStart::Refused => {}
        }
    }
    app.quit = if pending.is_empty() {
        crate::app::QuitNegotiation::Idle
    } else {
        crate::app::QuitNegotiation::SaveFanOut(QuitIntent { pending })
    };
}

fn handle_rename_collision_key(app: &mut App, key: KeyInput) {
    if !RENAME_REPLACE.answers(key) {
        return;
    }
    if !crate::rename::replace_allowed(app) {
        messages::error(app, "cannot replace \u{2014} recovery store unavailable");
        return;
    }
    crate::rename::replace_confirmed(app);
}

fn handle_disk_conflict_key(app: &mut App, doc: DocumentId, key: KeyInput, effects: &mut Effects) {
    if DISK_CONFLICT_SAVE.answers(key) {
        let already_in_flight = app
            .doc(doc)
            .is_some_and(super::document::Document::save_in_flight);
        let start = save::trigger_save(app, doc, SaveMode::Force, SaveOrigin::Guard, effects);
        if !already_in_flight && matches!(start, SaveStart::InFlight) {
            clear_guard(app);
        }
    } else if DISK_CONFLICT_DISCARD.answers(key) {
        if app.active != doc {
            workspace::switch_to(app, doc);
        }
        crate::merge::begin(app, MergeIntent::Discard, effects);
        clear_guard_if_begin_started(app, doc);
    } else if DISK_CONFLICT_MERGE.answers(key) {
        if app.active != doc {
            workspace::switch_to(app, doc);
        }
        crate::merge::begin(app, MergeIntent::Merge, effects);
        clear_guard_if_begin_started(app, doc);
    }
}

fn clear_guard_if_begin_started(app: &mut App, doc: DocumentId) {
    if matches!(&app.merge, MergeState::Pending { doc: d, .. } if *d == doc) {
        clear_guard(app);
    }
}

fn handle_trash_key(app: &mut App, path: PathBuf, key: KeyInput, effects: &mut Effects) {
    if !TRASH_YES.answers(key) {
        return;
    }
    clear_guard(app);
    crate::trash::confirm(app, path, effects);
}

#[cfg(test)]
#[path = "guard_tests.rs"]
mod tests;
