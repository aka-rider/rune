//! The enqueue side of [`crate::rename`], split out to keep it under the
//! §1.6 line budget: [`enqueue_rename`] (routes to the store's writer FIFO
//! or a plain `Cmd`), the two no-store `Cmd` factories it and [`bind_new`]
//! use ([`rename_cmd`], `create_cmd`), and [`bind_new`] itself (a pathless
//! draft's Enter, which routes through the store's bind-new path when
//! store-bound, else through `create_cmd`). `crate::rename` owns the state
//! machine these feed into and the `apply_outcome` match that resolves
//! their replies.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rune_db::RenameOutcome;
use rune_vfs::Vfs;

use crate::app::App;
use crate::document::DocumentId;
use crate::rename::{RenameState, Ticket};
use crate::runtime::{Cmd, CmdKind, Effects, Msg};
use crate::title;

/// Enqueues the rename on whichever route this document has: the `rune-db`
/// writer FIFO when it is store-bound, else a plain `Cmd` over the injected
/// `Vfs`. Reports and returns `None` on an enqueue failure.
pub(crate) fn enqueue_rename(
    app: &mut App,
    id: DocumentId,
    from: &Path,
    to: &Path,
    effects: &mut Effects,
) -> Option<Ticket> {
    if let Some(db_id) = app.doc(id).and_then(|d| d.db.as_ref()).map(|d| d.db_id)
        && let Some(db) = app.db.as_ref()
    {
        return match db.store.rename_file(db_id, from, to) {
            Ok(op_id) => {
                app.db_ops.insert(op_id, crate::db::PendingOp::new(id));
                Some(Ticket::Db(op_id))
            }
            Err(e) => {
                crate::materialize_ack::on_store_failure(app, e.to_string());
                None
            }
        };
    }

    let generation = app.next_rename_gen;
    app.next_rename_gen = app.next_rename_gen.wrapping_add(1);
    effects.cmds.push(rename_cmd(
        Arc::clone(&app.vfs),
        from.to_path_buf(),
        to.to_path_buf(),
        generation,
    ));
    Some(Ticket::Cmd(generation))
}

/// The no-store rename `Cmd`: `rename_excl` over the injected `Vfs`
/// (§1.4.1's no-clobber atomic publish, §1.4.9's single filesystem seam).
/// A collision comes back as `Collided` with the destination's stat — a
/// refusal, not an error.
///
/// There is no `rename_replace` counterpart: without a store there is
/// nowhere to durably capture the displaced bytes, and §1.4.10 does not
/// bend for convenience.
pub(crate) fn rename_cmd(
    vfs: Arc<dyn Vfs + Send + Sync>,
    from: PathBuf,
    to: PathBuf,
    generation: u32,
) -> Cmd {
    Cmd::new(CmdKind::Rename, move || {
        let result = match vfs.rename_excl(&from, &to) {
            Ok(()) => Ok(RenameOutcome::Renamed { to }),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => match vfs.stat(&to) {
                Ok(seen) => Ok(RenameOutcome::Collided { seen }),
                Err(e) => Err(e.to_string()),
            },
            Err(e) => Err(e.to_string()),
        };
        Some(Msg::RenameDone { generation, result })
    })
}

/// A pathless draft's Enter: a CREATE, not a rename.
///
/// Routed through `materialize`'s bind-new path when the document is
/// store-bound (its EEXIST branch already refuses correctly), else through
/// a dedicated exclusive-create `Cmd`. Either way a collision is a footer
/// refusal, **never** a `RenameCollision` guard: offering `[R]eplace` here
/// would overwrite a foreign file with a buffer that has no CAS baseline
/// (§1.4.7).
///
/// Writes no focus of its own: `begin` (the only caller) always runs inside
/// `App::set_focus`'s blur of the title to the Editor, which assigns the
/// focus itself once this returns.
pub(crate) fn bind_new(app: &mut App, id: DocumentId, stem: &str, effects: &mut Effects) {
    let dir = crate::explorer::initial_root(app);
    let path = dir.join(format!("{stem}.{}", title::MARKDOWN_EXT));

    if app.doc(id).and_then(|d| d.db.as_ref()).is_some() && app.db.is_some() {
        // The store route: `materialize(bind_new=true)` is an atomic
        // `rename_excl` create whose EEXIST branch refuses and records the
        // winner's bytes. `save::trigger_save` cannot be reused — it reads
        // `doc.file_path`, which is exactly what does not exist yet.
        crate::save::bind_new_now(app, id, path);
        return;
    }

    let bytes = app
        .doc(id)
        .map(|d| d.buffer.content().as_bytes().to_vec())
        .unwrap_or_default();
    let generation = app.next_rename_gen;
    app.next_rename_gen = app.next_rename_gen.wrapping_add(1);
    let path_for_state = path.clone();
    effects
        .cmds
        .push(create_cmd(Arc::clone(&app.vfs), path, bytes, generation));
    // `from` is EMPTY, and that is the discriminant: a create has no
    // source path, so `apply_outcome`'s `Collided` arm uses it to tell a
    // draft-create refusal (a footer message) from a rename collision (a
    // `[R]eplace` guard). §1.7 — the emptiness is the meaning, and it is
    // structurally unreachable for a rename, which always has a `from`.
    app.rename = RenameState::Committing {
        doc: id,
        from: PathBuf::new(),
        to: path_for_state,
        ticket: Ticket::Cmd(generation),
    };
}

/// The no-store draft-create `Cmd`: durable temp write, then a no-clobber
/// `rename_excl` publish (§1.4.1). On `AlreadyExists` the temp is genuinely
/// unneeded — the existing file is untouched and stays the winner — so it
/// is removed and the create refused.
fn create_cmd(
    vfs: Arc<dyn Vfs + Send + Sync>,
    path: PathBuf,
    bytes: Vec<u8>,
    generation: u32,
) -> Cmd {
    Cmd::new(CmdKind::Rename, move || {
        let result = (|| {
            let temp = vfs
                .write_durable(&path, &bytes)
                .map_err(|e| e.to_string())?;
            match vfs.rename_excl(&temp, &path) {
                Ok(()) => Ok(RenameOutcome::Renamed { to: path.clone() }),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let _ = vfs.remove(&temp);
                    match vfs.stat(&path) {
                        Ok(seen) => Ok(RenameOutcome::Collided { seen }),
                        Err(e) => Err(e.to_string()),
                    }
                }
                // Deliberately NOT removed: the publish never happened and
                // the temp is the only place these bytes physically exist
                // outside the buffer (materialize's own doctrine).
                Err(e) => Err(e.to_string()),
            }
        })();
        Some(Msg::RenameDone { generation, result })
    })
}
