use std::io;
use std::path::PathBuf;
use std::time::SystemTime;

use rusqlite::{Connection, params};

use rune_vfs::Vfs;

use crate::Error;
use crate::adopt;
use crate::bracket;
use crate::confirmation::Confirmation;
use crate::ids::{DocId, SessionId};
use crate::obs_origin::ObsOrigin;
use crate::observation;
use crate::retry;
use crate::sync::{self, SyncKind, SyncState, Version};

pub(crate) fn probe(
    conn: &mut Connection,
    vfs: &dyn Vfs,
    session_id: SessionId,
    doc_id: DocId,
    now: SystemTime,
) -> Result<SyncState, Error> {
    let path: String = retry::with_retry(conn, |tx| {
        tx.query_row(
            "SELECT path FROM documents WHERE id=?1",
            params![doc_id],
            |r| r.get(0),
        )
        .map_err(Error::from)
    })?;

    if path.is_empty() {
        return retry::with_retry(conn, |tx| sync::sync(tx, session_id, doc_id));
    }

    let path = PathBuf::from(path);
    let resolved = vfs.resolve(&path).map_err(Error::Io)?;

    if let Err(e) = vfs.stat(&resolved) {
        if e.kind() == io::ErrorKind::NotFound {
            return Err(Error::NotFound(format!(
                "probe doc {doc_id}: {}",
                path.display()
            )));
        }
        return Err(Error::Io(e));
    }

    let stat = observation::stat_identity(vfs, &resolved);
    let existing = retry::with_retry(conn, |tx| observation::newest_observation(tx, doc_id))?;
    let unchanged = existing.filter(|o| {
        o.confirmed == Confirmation::Confirmed
            && o.size == stat.size
            && o.mtime == stat.mtime
            && o.inode == stat.inode
            && o.device == stat.device
    });

    let theirs_obs = match unchanged {
        Some(obs) => obs,
        None => bracket::observe_disk(
            conn,
            vfs,
            session_id,
            doc_id,
            &resolved,
            bracket::ObserveDiskMeta {
                seq: None,
                origin: ObsOrigin::Probe,
            },
            now,
        )?,
    };

    let theirs = Some(Version {
        hash: theirs_obs.blob_hash.clone(),
        obs: Some(theirs_obs.id),
    });
    let state = retry::with_retry(conn, |tx| {
        sync::sync_with_theirs(tx, session_id, doc_id, theirs.clone())
    })?;

    if state.kind == SyncKind::Clean {
        let should_adopt = retry::with_retry(conn, |tx| {
            let cur = observation::saved_obs_for(tx, session_id, doc_id)?;
            Ok::<bool, Error>(match cur {
                None => true,
                Some(c) => c.blob_hash != theirs_obs.blob_hash,
            })
        })?;
        if should_adopt {
            let pos = retry::with_retry(conn, |tx| {
                crate::journal::current_seq(tx, session_id, doc_id)
            })?;
            let _ = adopt::adopt_equal(conn, session_id, doc_id, theirs_obs.id, pos.0, now)?;
        }
    }

    Ok(state)
}

#[cfg(test)]
#[path = "probe_tests.rs"]
mod tests;
