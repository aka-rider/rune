//! The caller-side publish half of the save flow: reacting to
//! `MaterializePrepare`'s ack — including the divergence gate that refuses
//! an ordinary publish outright — spawning the `vfs` `Cmd` that performs the
//! whole disk dance, and reacting to what that `Cmd` concluded
//! ([`MaterializeVfsOutcome`]). The parent module keeps the recording step
//! and the whole-store degrade; [`super::reactions`] keeps everything from
//! the recording ack onward.

use std::path::PathBuf;
use std::sync::Arc;

use rune_db::{MatResult, MaterializeOutcome, ObsOrigin, StatFacts, SyncKind};
use rune_vfs::Vfs;

use crate::app::App;
use crate::document::{DocumentId, PublishParams, SaveTicket};
use crate::messages;
use crate::runtime::{Cmd, CmdKind, Effects, Msg};
use crate::save::{self, SaveMode};

use super::reactions::{fail_materialize_locally, handle_materialize_ack};
use super::{
    RecordTarget, SAVE_REFUSED_DISK_CHANGED, raise_disk_conflict, record_orphan_outcome,
    record_outcome,
};

/// `Preparing`'s reaction: `prep` carries the materialize decision data
/// (`rune_db::MaterializePrep`) — no disk-sourced fact of its own — so this
/// only advances `id` from `Preparing` to `Publishing` and spawns the
/// caller-side `vfs` `Cmd` that performs the ENTIRE disk dance. A document
/// that has moved on since this op was enqueued (closed mid-flight, or a
/// stale ack for an attempt this document already abandoned) is a correct,
/// silent no-op — `op_id` is checked against the document's own `prep_op`
/// before anything else happens.
pub(crate) fn handle_prepare_ack(
    app: &mut App,
    id: DocumentId,
    op_id: u64,
    prep: rune_db::MaterializePrep,
    effects: &mut Effects,
) {
    if app.doc(id).and_then(|d| d.prep_op()) != Some(op_id) {
        return;
    }
    let (prep_expect_hash, bound_path) = match prep {
        rune_db::MaterializePrep::Create => (String::new(), None),
        rune_db::MaterializePrep::Overwrite {
            bound_path,
            expect_hash,
            sync,
        } => {
            if sync.is_disk_divergent()
                && app.doc(id).and_then(|d| d.preparing_mode()) == Some(SaveMode::Normal)
            {
                refuse_divergent_publish(app, id, sync);
                return;
            }
            (expect_hash.to_string(), Some(bound_path))
        }
    };
    // A baseline left unconfirmed by a prior commit whose observation was
    // lost (`FileBinding::pending_rebaseline_hash`'s own doc comment) stands
    // in for `expect_hash` here — the DB's own lookup would otherwise still
    // be answering off the stale row `expect_obs` never advanced past. Once
    // a real observation lands, this returns `None` again and the DB's own
    // hash is used as always. Shared per file, not per document: whichever
    // tab's own lost-bookkeeping commit produced the stash, this document's
    // OWN next save must recognize the same disk bytes as its own echo too.
    let expect_hash = app
        .doc_file_binding(id)
        .and_then(|b| b.pending_rebaseline_hash.clone())
        .unwrap_or(prep_expect_hash);
    let Some(doc) = app.doc_mut(id) else { return };
    let Some((ticket, content, params)) = doc.begin_publishing() else {
        return;
    };
    let vfs = Arc::clone(&app.vfs);
    effects.cmds.push(materialize_vfs_cmd(
        id,
        ticket,
        vfs,
        content,
        params,
        expect_hash,
        bound_path,
    ));
}

fn refuse_divergent_publish(app: &mut App, id: DocumentId, kind: SyncKind) {
    fail_materialize_locally(app, id, SAVE_REFUSED_DISK_CHANGED);
    raise_disk_conflict(app, id, kind);
}

/// What the caller-side `vfs` work ([`save::run_materialize_vfs`])
/// concluded — every disk-sourced fact [`handle_materialize_vfs_done`]
/// needs, carried so this module never has to call `vfs` a second time to
/// re-derive any of it.
#[derive(Debug)]
pub enum MaterializeVfsOutcome {
    /// The overwrite target no longer exists (`bind_new=false` only) —
    /// never silently (re)create.
    Missing,
    /// The caller's own target disagrees with the document's bound path —
    /// a caller-bug guard, not an ordinary CAS race. No `vfs` write was
    /// attempted.
    PathDisagreement,
    /// A genuine `vfs` I/O failure. No `rune-db` op is ever enqueued for
    /// this outcome — nothing happened worth recording, and the failure is
    /// specific to this document's save, not the store.
    Error(String),
    /// The live target (or, for `bind_new`, a concurrent creator's file)
    /// didn't match `expect` — no write was attempted; `data`/`stat`
    /// describe whatever is actually on disk now. `confirmed` is the
    /// bracketed read's own verdict — a racer caught mid-external-rewrite
    /// must never masquerade as a stable fact.
    Conflict {
        data: Vec<u8>,
        origin: ObsOrigin,
        stat: StatFacts,
        confirmed: bool,
        resolved_path: PathBuf,
    },
    /// The write committed with no race. `confirmed` is the post-publish
    /// stat's own verdict; `durable: false` means the publish took effect
    /// but its durability confirmation failed — still a success, surfaced
    /// as a warning.
    Committed {
        data: Vec<u8>,
        stat: StatFacts,
        confirmed: bool,
        resolved_path: PathBuf,
        durable: bool,
    },
    /// The write committed AND a racer's displaced bytes were captured in
    /// the same atomic-swap window. `confirmed` describes `stat` only.
    Raced {
        data: Vec<u8>,
        stat: StatFacts,
        confirmed: bool,
        displaced: Vec<u8>,
        displaced_stat: StatFacts,
        resolved_path: PathBuf,
        durable: bool,
    },
}

/// `Publishing`'s own vfs `Cmd` — resolves the destination, CAS-checks it
/// (`!bind_new`), publishes (`exchange`/`rename_excl`), and on a plain
/// overwrite, reads back the displaced bytes to detect a swap-race —
/// entirely through THIS app's own `Vfs` handle, never the writer thread's.
/// `db_id`/`seq`/`content` are captured here at spawn time and echoed back
/// on `Msg::MaterializeVfsDone` — never re-read from the document once this
/// `Cmd` is running, so a `Committed`/`Raced` outcome can still be recorded
/// durably even if the document has since closed (`record_orphan_outcome`).
/// Tagged `CmdKind::Save` (not a new kind) so quit's existing `save_handles`
/// join covers it exactly like the no-store fallback save.
fn materialize_vfs_cmd(
    id: DocumentId,
    ticket: SaveTicket,
    vfs: Arc<dyn Vfs + Send + Sync>,
    content: Arc<str>,
    params: PublishParams,
    expect_hash: String,
    bound_path: Option<String>,
) -> Cmd {
    Cmd::new(CmdKind::Save, move || {
        let outcome = save::run_materialize_vfs(
            vfs.as_ref(),
            &params.path,
            params.bind_new,
            &content,
            &expect_hash,
            bound_path.as_deref(),
            params.mode,
        );
        Some(Msg::MaterializeVfsDone {
            id,
            ticket,
            db_id: params.db_id,
            seq: params.seq,
            content,
            outcome,
        })
    })
}

/// `Publishing`'s reaction: reacts to [`MaterializeVfsOutcome`]. `live` is
/// `true` only when `id` is still `Publishing` on exactly `ticket` — a
/// document that closed, or moved on to a later attempt, mid-flight gets a
/// typed, silent drop for every outcome that never touched disk
/// (`Missing`/`Error`/`Conflict`), but a `Committed`/`Raced` write already
/// took effect regardless of whether anything is still listening, so its
/// bytes are still recorded durably via [`record_orphan_outcome`] — bytes a
/// write displaces are captured before anything discards them, live
/// document or not.
pub(crate) fn handle_materialize_vfs_done(
    app: &mut App,
    id: DocumentId,
    ticket: SaveTicket,
    db_id: i64,
    seq: i64,
    content: Arc<str>,
    outcome: MaterializeVfsOutcome,
) {
    let live = app
        .doc(id)
        .is_some_and(|d| d.save_ticket() == Some(ticket) && d.is_publishing());
    match outcome {
        MaterializeVfsOutcome::Missing => {
            if live {
                handle_materialize_ack(app, id, MatResult::Missing);
            }
        }
        MaterializeVfsOutcome::PathDisagreement => {
            super::on_store_failure(
                app,
                "materialize refused: caller-supplied path does not match the bound path"
                    .to_string(),
            );
        }
        MaterializeVfsOutcome::Error(e) => {
            if live {
                fail_materialize_locally(app, id, format!("save failed: {e}"));
            }
        }
        MaterializeVfsOutcome::Conflict {
            data,
            origin,
            stat,
            confirmed,
            resolved_path,
        } => {
            if live {
                record_outcome(
                    app,
                    id,
                    RecordTarget {
                        db_id,
                        seq,
                        content: &content,
                        resolved_path: &resolved_path,
                    },
                    MaterializeOutcome::Conflict {
                        data,
                        origin,
                        stat,
                        confirmed,
                    },
                    false,
                );
            }
        }
        MaterializeVfsOutcome::Committed {
            data,
            stat,
            confirmed,
            resolved_path,
            durable,
        } => {
            if !durable {
                messages::warn(app, super::DURABILITY_UNCONFIRMED_WARNING);
            }
            let outcome = MaterializeOutcome::Committed {
                data,
                stat,
                confirmed,
            };
            if live {
                record_outcome(
                    app,
                    id,
                    RecordTarget {
                        db_id,
                        seq,
                        content: &content,
                        resolved_path: &resolved_path,
                    },
                    outcome,
                    true,
                );
            } else {
                record_orphan_outcome(app, db_id, seq, &resolved_path, outcome);
            }
        }
        MaterializeVfsOutcome::Raced {
            data,
            stat,
            confirmed,
            displaced,
            displaced_stat,
            resolved_path,
            durable,
        } => {
            if !durable {
                messages::warn(app, super::DURABILITY_UNCONFIRMED_WARNING);
            }
            let outcome = MaterializeOutcome::Raced {
                data,
                stat,
                confirmed,
                displaced,
                displaced_stat,
            };
            if live {
                record_outcome(
                    app,
                    id,
                    RecordTarget {
                        db_id,
                        seq,
                        content: &content,
                        resolved_path: &resolved_path,
                    },
                    outcome,
                    true,
                );
            } else {
                record_orphan_outcome(app, db_id, seq, &resolved_path, outcome);
            }
        }
    }
}
