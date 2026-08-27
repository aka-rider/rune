use std::path::{Path, PathBuf};
use std::sync::Arc;

use rune_db::RenameOutcome;
use rune_vfs::Vfs;

use crate::app::App;
use crate::document::DocumentId;
use crate::rename::{Commit, RenameState, Ticket};
use crate::runtime::{Cmd, CmdError, Effects, Msg};
use crate::save::gate::{self, SaveEntry};

pub(crate) fn enqueue_rename(
    app: &mut App,
    id: DocumentId,
    from: &Path,
    to: &Path,
    effects: &mut Effects,
) -> Option<Ticket> {
    if let Some(db_id) = app.doc(id).and_then(|d| d.doc_db()).map(|d| d.db_id)
        && let Some(db) = app.db.as_ref()
    {
        return match db.store.rename_file(rune_db::DocId(db_id), from, to) {
            Ok(op_id) => {
                app.db_ops.insert(op_id, crate::db::PendingOp::new(id));
                Some(Ticket::Db(op_id))
            }
            Err(e) => {
                crate::materialize_ack::on_store_failure(app, &e.to_string());
                None
            }
        };
    }

    let generation = app.next_rename_gen.mint();
    effects.cmds.push(rename_cmd(
        Arc::clone(&app.vfs),
        from.to_path_buf(),
        to.to_path_buf(),
        generation,
    ));
    Some(Ticket::Cmd(generation))
}

/// No `rename_replace` counterpart here: without a store there is nowhere
/// to durably capture the displaced bytes, and that guarantee does not
/// bend for convenience.
pub(crate) fn rename_cmd(
    vfs: Arc<dyn Vfs + Send + Sync>,
    from: PathBuf,
    to: PathBuf,
    generation: crate::generation::RenameGen,
) -> Cmd {
    Cmd::rename(move || {
        let result = match vfs.rename_excl(&from, &to) {
            Ok(()) => Ok(RenameOutcome::Renamed { to, durable: true }),
            Err(e) if rune_vfs::published_not_durable(&e) => {
                Ok(RenameOutcome::Renamed { to, durable: false })
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => match vfs.stat(&to) {
                Ok(seen) => Ok(RenameOutcome::Collided { seen }),
                Err(e) => Err(CmdError::Io(e)),
            },
            Err(e) => Err(CmdError::Io(e)),
        };
        Some(Msg::RenameDone { generation, result })
    })
}

/// A pathless draft's Enter is a CREATE, not a rename: a collision here is
/// always a footer refusal, never a `RenameCollision` guard — offering
/// `[R]eplace` would overwrite a foreign file with a buffer that has no CAS
/// baseline.
pub(crate) fn bind_new(app: &mut App, id: DocumentId, name: &str, effects: &mut Effects) -> Commit {
    let Ok(clearance) = gate::clear(app, id, SaveEntry::BindNew) else {
        return Commit::Refused;
    };
    let dir = crate::explorer::initial_root(app);
    let path = dir.join(name);

    if app.doc(id).and_then(|d| d.doc_db()).is_some() && app.db.is_some() {
        // `save::trigger_save` reads `doc.file_path`, which is exactly
        // what does not exist yet, so it cannot be reused here — a
        // create-only `materialize` is instead an atomic `rename_excl`
        // create whose EEXIST branch refuses and records the winner's
        // bytes.
        crate::save::bind_new_now(app, id, path, &clearance);
        return Commit::Accepted;
    }

    crate::commands::strip_trailing::leave_reading_then_strip(app, id);
    let (version, content) = app.doc(id).map_or_else(
        || (0, Arc::from("")),
        |d| (d.buffer.version(), Arc::<str>::from(d.buffer.content())),
    );
    let bytes = content.as_bytes().to_vec();
    let generation = app.next_rename_gen.mint();
    let path_for_state = path.clone();
    effects
        .cmds
        .push(create_cmd(Arc::clone(&app.vfs), path, bytes, generation));
    // `from` is EMPTY, and that is the discriminant `apply_outcome`'s
    // `Collided` arm uses to tell a draft-create refusal from a rename
    // collision — structurally unreachable for a rename, which always has
    // a `from`.
    app.rename = RenameState::Committing {
        doc: id,
        from: PathBuf::new(),
        to: path_for_state,
        ticket: Ticket::Cmd(generation),
        draft_baseline: Some(crate::document::SaveCapture { version, content }),
    };
    Commit::Accepted
}

/// A no-clobber `rune_vfs::put` (`IfAbsent` — durable temp write, then
/// `rename_excl`). A losing race comes back as `Collided` with the
/// winner's stat (the temp is removed); a non-collision publish failure
/// keeps the temp — the only place these bytes physically exist outside
/// the buffer.
fn create_cmd(
    vfs: Arc<dyn Vfs + Send + Sync>,
    path: PathBuf,
    bytes: Vec<u8>,
    generation: crate::generation::RenameGen,
) -> Cmd {
    Cmd::rename(move || {
        let result = match rune_vfs::put(
            vfs.as_ref(),
            &path,
            &bytes,
            rune_vfs::PutCondition::IfAbsent,
        ) {
            Ok(rune_vfs::PutOutcome::Committed { durable, .. }) => Ok(RenameOutcome::Renamed {
                to: path.clone(),
                durable,
            }),
            Ok(rune_vfs::PutOutcome::Conflict { current, .. }) => {
                current.sighted.stat().map_or_else(
                    || Err(CmdError::Refused("target already exists".to_string())),
                    |seen| Ok(RenameOutcome::Collided { seen }),
                )
            }
            Ok(_) => Err(CmdError::Refused(
                "create failed: unexpected publish outcome".to_string(),
            )),
            Err(e) => Err(CmdError::Io(e)),
        };
        Some(Msg::RenameDone { generation, result })
    })
}
