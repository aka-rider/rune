use std::path::{Path, PathBuf};
use std::sync::Arc;

use rune_db::RenameOutcome;
use rune_vfs::Vfs;

use crate::app::App;
use crate::document::{DocumentId, Replica};
use crate::messages;
use crate::rename::{Commit, RenameState, Ticket};
use crate::resolved::ResolvedPath;
use crate::runtime::{Cmd, CmdError, Effects, Msg};
use crate::save::gate::{self, SaveEntry};

enum Route {
    Now(Via),
    AfterBind,
}

#[derive(Clone, Copy)]
enum Via {
    Store { db_id: i64, create_only: bool },
    Plain,
}

fn route(app: &App, id: DocumentId) -> Option<Route> {
    let replica = &app.doc(id)?.replica;
    Some(match (replica, app.db.is_some()) {
        (_, false) | (Replica::Detached, true) => Route::Now(Via::Plain),
        (Replica::Binding { .. }, true) => Route::AfterBind,
        (Replica::Bound(doc_db), true) => Route::Now(Via::Store {
            db_id: doc_db.db_id,
            create_only: doc_db.publish_mode.is_create_only(),
        }),
    })
}

pub(crate) fn commit_or_await(
    app: &mut App,
    id: DocumentId,
    to: ResolvedPath,
    effects: &mut Effects,
) -> Commit {
    match route(app, id) {
        None => Commit::Accepted,
        Some(Route::Now(via)) => commit_now(app, id, to, via, effects),
        Some(Route::AfterBind) => {
            let name = crate::rename::display_name(to.as_path());
            let verb = match app.doc(id).and_then(|doc| doc.path()) {
                Some(_) => "renaming to",
                None => "creating",
            };
            messages::info(
                app,
                format!("{verb} {name} once crash recovery is ready\u{2026}"),
            );
            app.rename = RenameState::AwaitingBind { doc: id, to };
            Commit::Accepted
        }
    }
}

pub(crate) fn settle_awaiting_bind(app: &mut App, effects: &mut Effects) {
    let RenameState::AwaitingBind { doc, to } = &app.rename else {
        return;
    };
    let (id, to) = (*doc, to.clone());
    let route = route(app, id);
    if matches!(route, Some(Route::AfterBind)) {
        return;
    }
    app.rename = RenameState::Idle;
    let Some(Route::Now(via)) = route else {
        return;
    };
    if commit_now(app, id, to, via, effects) == Commit::Refused {
        crate::rename::return_to_title(app, id);
    }
}

fn commit_now(
    app: &mut App,
    id: DocumentId,
    to: ResolvedPath,
    via: Via,
    effects: &mut Effects,
) -> Commit {
    let Some(from) = app
        .doc(id)
        .and_then(|doc| doc.path())
        .map(Path::to_path_buf)
    else {
        return create_now(app, id, to, via, effects);
    };
    if let Via::Store {
        create_only: true, ..
    } = via
    {
        let Ok(clearance) = gate::clear(app, id, SaveEntry::BindNew) else {
            return Commit::Refused;
        };
        crate::save::bind_new_now(app, id, to, &clearance);
        return Commit::Accepted;
    }
    if let Some(ticket) = enqueue_rename(app, id, &from, &to, via, effects) {
        app.rename = RenameState::Committing {
            doc: id,
            from,
            to,
            ticket,
            draft_baseline: None,
        };
    }
    Commit::Accepted
}

fn enqueue_rename(
    app: &mut App,
    id: DocumentId,
    from: &Path,
    to: &Path,
    via: Via,
    effects: &mut Effects,
) -> Option<Ticket> {
    if let Via::Store { db_id, .. } = via
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
            Ok(()) => Ok(RenameOutcome::Renamed { durable: true }),
            Err(e) if rune_vfs::published_not_durable(&e) => {
                Ok(RenameOutcome::Renamed { durable: false })
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
    if gate::clear(app, id, SaveEntry::BindNew).is_err() {
        return Commit::Refused;
    }
    let dir = crate::explorer::initial_root(app);
    let Some(path) = crate::workspace::resolve_or_report(app, &dir.join(name), "create") else {
        return Commit::Refused;
    };
    commit_or_await(app, id, path, effects)
}

fn create_now(
    app: &mut App,
    id: DocumentId,
    path: ResolvedPath,
    via: Via,
    effects: &mut Effects,
) -> Commit {
    let Ok(clearance) = gate::clear(app, id, SaveEntry::BindNew) else {
        return Commit::Refused;
    };
    match via {
        Via::Store { .. } => crate::save::bind_new_now(app, id, path, &clearance),
        Via::Plain => create_plain(app, id, path, effects),
    }
    Commit::Accepted
}

fn create_plain(app: &mut App, id: DocumentId, path: ResolvedPath, effects: &mut Effects) {
    crate::commands::strip_trailing::leave_reading_then_strip(app, id);
    let (version, content) = app.doc(id).map_or_else(
        || (0, Arc::from("")),
        |d| (d.buffer().version(), Arc::<str>::from(d.buffer().content())),
    );
    let bytes = rune_core::codec::encode(&content).into_owned();
    let generation = app.next_rename_gen.mint();

    effects.cmds.push(create_cmd(
        Arc::clone(&app.vfs),
        path.as_path().to_path_buf(),
        bytes,
        generation,
    ));
    // `from` is EMPTY, and that is the discriminant `apply_outcome`'s
    // `Collided` arm uses to tell a draft-create refusal from a rename
    // collision — structurally unreachable for a rename, which always has
    // a `from`.
    app.rename = RenameState::Committing {
        doc: id,
        from: PathBuf::new(),
        to: path,
        ticket: Ticket::Cmd(generation),
        draft_baseline: Some(crate::document::SaveCapture { version, content }),
    };
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
            Ok(rune_vfs::PutOutcome::Committed { durable, .. }) => {
                Ok(RenameOutcome::Renamed { durable })
            }
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
