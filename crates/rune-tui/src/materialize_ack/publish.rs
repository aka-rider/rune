//! The caller-side publish half of the save flow: reacting to
//! `MaterializePrepare`'s ack — including the divergence gate that refuses
//! an ordinary publish outright — spawning the `vfs` `Cmd` that performs the
//! whole disk dance, and reacting to what that `Cmd` concluded
//! ([`MaterializeVfsOutcome`]). The parent module keeps the recording step
//! and the whole-store degrade; [`super::reactions`] keeps everything from
//! the recording ack onward.

use std::path::PathBuf;
use std::sync::Arc;

use rune_db::{MaterializeOutcome, ObsOrigin, SyncKind};
use rune_vfs::{PutOutcome, Vfs};

use crate::app::App;
use crate::document::{DocumentId, PublishParams, SaveTicket};
use crate::messages;
use crate::runtime::{Cmd, Effects, Msg};
use crate::save::{self, SaveMode};

use super::reactions::fail_materialize_locally;
use super::{
    RecordTarget, SAVE_REFUSED_DISK_CHANGED, raise_disk_conflict, record_orphan_outcome,
    record_outcome,
};

pub(crate) const SAVE_RACED_BASELINE_REWRITE: &str =
    "save cancelled \u{2014} the document was updated while the save was starting; save again";

/// `Preparing`'s reaction: `prep` carries the materialize decision data
/// (`rune_db::MaterializePrep`) — no disk-sourced fact of its own — so this
/// only advances `id` from `Preparing` to `Publishing` and spawns the
/// caller-side `vfs` `Cmd` that performs the ENTIRE disk dance. A document
/// that has moved on since this op was enqueued (closed mid-flight, or a
/// stale ack for an attempt this document already abandoned) is a correct,
/// silent no-op — `op_id` is checked against the document's own `prep_op`
/// before anything else happens; `enqueue_epoch` is then checked against
/// the file binding's CURRENT `baseline_epoch` — a baseline rewrite in
/// flight (an adoption, an abandon, a sibling tab's publish) means every
/// verdict and hash this prep carries describes a world that no longer
/// exists, so the attempt is abandoned with feedback and a fresh probe
/// re-classifies the document instead (the probe ack's own stale-epoch
/// drop, applied to the save path).
pub(crate) fn handle_prepare_ack(
    app: &mut App,
    id: DocumentId,
    op_id: u64,
    enqueue_epoch: Option<u32>,
    prep: rune_db::MaterializePrep,
    effects: &mut Effects,
) {
    if app
        .doc(id)
        .and_then(super::super::document::Document::prep_op)
        != Some(op_id)
    {
        return;
    }
    if enqueue_epoch.is_some()
        && enqueue_epoch != app.doc_file_binding(id).map(|b| b.baseline_epoch)
    {
        fail_materialize_locally(app, id, SAVE_RACED_BASELINE_REWRITE);
        crate::db_enqueue::probe(app, id);
        return;
    }
    let (expect_hash, bound_path) = match prep {
        rune_db::MaterializePrep::Create => (String::new(), None),
        rune_db::MaterializePrep::Overwrite {
            bound_path,
            expect_hash,
            sync,
        } => {
            if sync.is_disk_divergent()
                && app
                    .doc(id)
                    .and_then(super::super::document::Document::preparing_mode)
                    == Some(SaveMode::Normal)
            {
                refuse_divergent_publish(app, id, sync, effects);
                return;
            }
            (expect_hash.to_string(), Some(bound_path))
        }
    };
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

fn refuse_divergent_publish(app: &mut App, id: DocumentId, kind: SyncKind, effects: &mut Effects) {
    fail_materialize_locally(app, id, SAVE_REFUSED_DISK_CHANGED);
    raise_disk_conflict(app, id, kind, effects);
}

/// What the caller-side `vfs` work ([`save::run_materialize_vfs`])
/// concluded. `Put` carries the real `rune_vfs::PutOutcome` through
/// untranslated, alongside the resolved target path that `PutOutcome`
/// itself never carries. `PathDisagreement`/`Error` are pre-`vfs::put`
/// refusals `PutOutcome` has no variant for.
#[derive(Debug)]
pub enum MaterializeVfsOutcome {
    /// The caller's own target disagrees with the document's bound path —
    /// a caller-bug guard, not an ordinary CAS race. No `vfs` write was
    /// attempted.
    PathDisagreement,
    /// A genuine `vfs` I/O failure. No `rune-db` op is ever enqueued for
    /// this outcome — nothing happened worth recording, and the failure is
    /// specific to this document's save, not the store.
    Error(String),
    /// The `vfs` write was attempted and concluded — `outcome` is exactly
    /// what `rune_vfs::put` reported.
    Put {
        resolved_path: PathBuf,
        outcome: Box<PutOutcome>,
    },
}

/// `Publishing`'s own vfs `Cmd` — resolves the destination, CAS-checks it
/// (overwrite publishes only), publishes (`exchange`/`rename_excl`), and on a plain
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
    Cmd::save(move || {
        let outcome = save::run_materialize_vfs(
            vfs.as_ref(),
            &params.path,
            params.publish_mode,
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
    content: &Arc<str>,
    outcome: MaterializeVfsOutcome,
) {
    let live = app
        .doc(id)
        .is_some_and(|d| d.save_ticket() == Some(ticket) && d.is_publishing());
    match outcome {
        MaterializeVfsOutcome::PathDisagreement => {
            super::on_store_failure(
                app,
                "materialize refused: caller-supplied path does not match the bound path",
            );
        }
        MaterializeVfsOutcome::Error(e) => {
            if live {
                fail_materialize_locally(app, id, format!("save failed: {e}"));
            }
        }
        MaterializeVfsOutcome::Put {
            resolved_path,
            outcome,
        } => handle_put_outcome(
            app,
            id,
            live,
            RecordTarget {
                db_id,
                seq,
                content,
                resolved_path: &resolved_path,
            },
            *outcome,
        ),
    }
}

fn handle_put_outcome(
    app: &mut App,
    id: DocumentId,
    live: bool,
    target: RecordTarget<'_>,
    outcome: PutOutcome,
) {
    let RecordTarget {
        db_id,
        seq,
        content,
        resolved_path,
    } = target;
    match outcome {
        PutOutcome::Missing => {
            if live {
                super::reactions::resolve_missing_ack(app, id);
            }
        }
        PutOutcome::Conflict { current, .. } => {
            if live {
                record_outcome(
                    app,
                    id,
                    target,
                    MaterializeOutcome::Conflict {
                        confirmed: current.sighted.is_confirmed(),
                        stat: rune_db::stat_facts_from(current.sighted.stat()),
                        origin: ObsOrigin::Probe,
                        data: current.bytes,
                    },
                    false,
                );
            }
        }
        PutOutcome::Committed {
            sighted,
            durable,
            stray_temp,
            ..
        } => {
            if !durable {
                messages::warn(app, super::DURABILITY_UNCONFIRMED_WARNING);
            }
            if let Some(temp) = &stray_temp {
                messages::warn(app, super::stray_temp_warning(temp));
            }
            let outcome = MaterializeOutcome::Committed {
                data: content.as_bytes().to_vec(),
                confirmed: sighted.is_confirmed(),
                stat: rune_db::stat_facts_from(sighted.stat()),
            };
            if live {
                record_outcome(app, id, target, outcome, true);
            } else {
                record_orphan_outcome(app, db_id, seq, resolved_path, outcome);
            }
        }
        PutOutcome::Raced {
            sighted,
            durable,
            displaced,
            stray_temp,
            ..
        } => {
            if !durable {
                messages::warn(app, super::DURABILITY_UNCONFIRMED_WARNING);
            }
            if let Some(temp) = &stray_temp {
                messages::warn(app, super::stray_temp_warning(temp));
            }
            let outcome = MaterializeOutcome::Raced {
                data: content.as_bytes().to_vec(),
                confirmed: sighted.is_confirmed(),
                stat: rune_db::stat_facts_from(sighted.stat()),
                displaced_stat: rune_db::stat_facts_from(displaced.sighted.stat()),
                displaced: displaced.bytes,
            };
            if live {
                record_outcome(app, id, target, outcome, true);
            } else {
                record_orphan_outcome(app, db_id, seq, resolved_path, outcome);
            }
        }
    }
}
