use rusqlite::{Connection, Transaction, params};

use std::time::SystemTime;

use crate::Error;
use crate::ids::{DocId, SessionId};
use crate::inherit::most_recent_session_for_doc;
use crate::retry;
use crate::session::parse_rfc3339_nanos;

pub fn reap_dead_sessions(
    conn: &mut Connection,
    is_alive: &dyn Fn(i64, &str) -> bool,
    boot: Option<SystemTime>,
) -> Result<(), Error> {
    let candidates: Vec<(SessionId, i64, String, String)> = {
        let mut stmt = conn.prepare("SELECT id, pid, proc_started_at, opened_at FROM sessions")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
        rows.collect::<Result<Vec<_>, _>>()?
    };

    for (id, pid, started_at, opened_at) in candidates {
        let dead_since_reboot = started_at.is_empty() && predates_boot(&opened_at, boot);
        if !dead_since_reboot && is_alive(pid, &started_at) {
            continue;
        }
        let reapable = retry::with_retry(conn, |tx| session_is_reapable(tx, id))?;
        if !reapable {
            continue;
        }
        retry::with_retry(conn, |tx| reap_session_footprint(tx, id))?;
    }
    Ok(())
}

fn predates_boot(opened_at: &str, boot: Option<std::time::SystemTime>) -> bool {
    match (parse_rfc3339_nanos(opened_at), boot) {
        (Some(opened), Some(boot)) => opened < boot,
        _ => false,
    }
}

fn session_is_reapable(tx: &Transaction<'_>, session_id: SessionId) -> Result<bool, Error> {
    let doc_ids: Vec<DocId> = {
        let mut stmt = tx.prepare(
            "SELECT DISTINCT doc_id FROM ( \
                SELECT doc_id FROM events    WHERE session_id=?1 \
                UNION \
                SELECT doc_id FROM snapshots WHERE session_id=?1 \
             )",
        )?;
        let rows = stmt.query_map(params![session_id], |r| r.get(0))?;
        rows.collect::<Result<Vec<_>, _>>()?
    };

    for doc_id in doc_ids {
        if let Some(most_recent) = most_recent_session_for_doc(tx, doc_id)?
            && most_recent == session_id
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn reap_session_footprint(tx: &Transaction<'_>, session_id: SessionId) -> Result<(), Error> {
    tx.execute(
        "DELETE FROM session_documents WHERE session_id=?1",
        params![session_id],
    )?;
    tx.execute(
        "DELETE FROM events WHERE session_id=?1",
        params![session_id],
    )?;
    tx.execute(
        "DELETE FROM snapshots WHERE session_id=?1",
        params![session_id],
    )?;
    tx.execute(
        "DELETE FROM sessions WHERE id=?1 \
         AND NOT EXISTS(SELECT 1 FROM observations WHERE session_id=?1) \
         AND NOT EXISTS(SELECT 1 FROM merges WHERE session_id=?1)",
        params![session_id],
    )?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::test_support::open;
    use std::time::SystemTime;

    fn seed_doc(conn: &Connection) -> DocId {
        conn.execute(
            "INSERT INTO documents(path, created_at, last_seen_at) VALUES ('', 'x', 'x')",
            [],
        )
        .expect("seed doc");
        DocId(conn.last_insert_rowid())
    }

    fn seed_session(conn: &Connection, pid: i64) -> SessionId {
        conn.execute(
            "INSERT INTO sessions(pid, proc_started_at, opened_at) VALUES(?1, 'started', 'opened')",
            params![pid],
        )
        .expect("seed session");
        SessionId(conn.last_insert_rowid())
    }

    fn seed_blob(conn: &Connection, hash: &str) {
        conn.execute(
            "INSERT INTO blobs(hash, content) VALUES(?1, ?2)",
            params![hash, b"x".as_slice()],
        )
        .expect("seed blob");
    }

    fn seed_observation(conn: &Connection, session_id: SessionId, doc_id: DocId) {
        let hash = format!("hash-{session_id}-{doc_id}");
        seed_blob(conn, &hash);
        conn.execute(
            "INSERT INTO observations(doc_id, session_id, blob_hash, origin, at) \
             VALUES(?1, ?2, ?3, 'probe', 'x')",
            params![doc_id, session_id, hash],
        )
        .expect("seed observation");
    }

    fn seed_merges_row(
        conn: &Connection,
        merges_session_id: SessionId,
        theirs_owner_session_id: SessionId,
        doc_id: DocId,
    ) {
        let obs_hash = format!("theirs-{merges_session_id}-{doc_id}");
        seed_blob(conn, &obs_hash);
        conn.execute(
            "INSERT INTO observations(doc_id, session_id, blob_hash, origin, at) \
             VALUES(?1, ?2, ?3, 'probe', 'x')",
            params![doc_id, theirs_owner_session_id, obs_hash],
        )
        .expect("seed theirs observation");
        let theirs_obs = conn.last_insert_rowid();

        let marker_hash = format!("marker-{merges_session_id}-{doc_id}");
        seed_blob(conn, &marker_hash);
        conn.execute(
            "INSERT INTO merges(doc_id, session_id, base_obs, theirs_obs, marker_hash, blocks, state, created_at) \
             VALUES(?1, ?2, NULL, ?3, ?4, '[]', 'active', 'x')",
            params![doc_id, merges_session_id, theirs_obs, marker_hash],
        )
        .expect("seed merges row");
    }

    fn journal_one_edit(conn: &mut Connection, session_id: SessionId, doc_id: DocId) {
        let tx = conn.transaction().expect("tx");
        crate::journal::append_edit(
            &tx,
            session_id,
            SystemTime::now(),
            doc_id,
            &[rune_core::buffer::AppliedEdit {
                start: 0,
                end: 0,
                deleted: String::new(),
                insert: "x".to_string(),
            }],
            &[],
            &[],
        )
        .expect("append_edit");
        tx.commit().expect("commit");
    }

    fn sessions_row_exists(conn: &Connection, session_id: SessionId) -> bool {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sessions WHERE id=?1)",
            params![session_id],
            |r| r.get(0),
        )
        .expect("check")
    }

    #[test]
    fn reaper_deletes_footprint_but_spares_sessions_row_with_an_observation() {
        let mut conn = open();
        let session_old = seed_session(&conn, 111);
        let doc_id = seed_doc(&conn);
        journal_one_edit(&mut conn, session_old, doc_id);
        seed_observation(&conn, session_old, doc_id);

        let session_new = seed_session(&conn, 222);
        journal_one_edit(&mut conn, session_new, doc_id);

        reap_dead_sessions(&mut conn, &|pid, _| pid != 111, None).expect("reap");

        let old_events: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM events WHERE session_id=?1",
                params![session_old],
                |r| r.get(0),
            )
            .expect("count");
        assert_eq!(
            old_events, 0,
            "superseded dead session's footprint must be reaped"
        );
        assert!(
            sessions_row_exists(&conn, session_old),
            "a sessions row that recorded an observation must never be deleted"
        );
    }

    #[test]
    fn reaper_deletes_the_sessions_row_of_an_observation_free_superseded_dead_session() {
        let mut conn = open();
        let session_old = seed_session(&conn, 111);
        let doc_id = seed_doc(&conn);
        journal_one_edit(&mut conn, session_old, doc_id);

        let session_new = seed_session(&conn, 222);
        journal_one_edit(&mut conn, session_new, doc_id);

        reap_dead_sessions(&mut conn, &|pid, _| pid != 111, None).expect("reap");

        assert!(
            !sessions_row_exists(&conn, session_old),
            "an observation-free sessions row must be reaped alongside its footprint"
        );
        assert!(
            sessions_row_exists(&conn, session_new),
            "the live session's own row must never be touched"
        );
    }

    #[test]
    fn reaper_spares_the_most_recent_dead_session() {
        let mut conn = open();
        let session_id = seed_session(&conn, 111);
        let doc_id = seed_doc(&conn);
        journal_one_edit(&mut conn, session_id, doc_id);

        reap_dead_sessions(&mut conn, &|_pid, _started_at| false, None).expect("reap");

        let events: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM events WHERE session_id=?1",
                params![session_id],
                |r| r.get(0),
            )
            .expect("count");
        assert_eq!(
            events, 1,
            "the most-recent dead session's footprint must survive"
        );
    }

    #[test]
    fn reaper_never_touches_a_live_session() {
        let mut conn = open();
        let session_id = seed_session(&conn, 111);
        let doc_id = seed_doc(&conn);
        journal_one_edit(&mut conn, session_id, doc_id);
        let session_new = seed_session(&conn, 222);
        journal_one_edit(&mut conn, session_new, doc_id);

        reap_dead_sessions(&mut conn, &|_pid, _started_at| true, None).expect("reap");

        let events: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM events WHERE session_id=?1",
                params![session_id],
                |r| r.get(0),
            )
            .expect("count");
        assert_eq!(events, 1, "a live session must never be reaped");
    }

    #[test]
    fn diverged_load_bridge_survives_reaping_the_dead_session_it_inherited_from() {
        use rune_vfs::{Vfs, VfsTestExt};

        let mut conn = open();
        let vfs = rune_vfs::Mem::new();
        let path = std::path::Path::new("/doc.md");
        let publish = |bytes: &[u8]| {
            let temp = vfs.write_durable(path, bytes).expect("write_durable");
            vfs.rename_excl(&temp, path).expect("publish");
        };
        publish(b"session A's content");

        let session_a =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session a");
        let doc_id = crate::load::load(
            &mut conn,
            &vfs,
            session_a,
            &|_pid, _started_at| true,
            path,
            SystemTime::now(),
        )
        .expect("session a load")
        .doc_id;

        {
            let tx = conn.transaction().expect("tx");
            crate::journal::append_edit(
                &tx,
                session_a,
                SystemTime::now(),
                doc_id,
                &[rune_core::buffer::AppliedEdit {
                    start: 0,
                    end: 0,
                    deleted: String::new(),
                    insert: "UNSAVED ".to_string(),
                }],
                &[],
                &[],
            )
            .expect("append_edit");
            tx.commit().expect("commit");
        }

        vfs.save_atomic(path, b"disk moved on independently")
            .expect("external atomic swap");

        let session_b =
            crate::session::establish_session(&conn, SystemTime::now()).expect("session b");
        let result = crate::load::load(
            &mut conn,
            &vfs,
            session_b,
            &|_pid, _started_at| false,
            path,
            SystemTime::now(),
        )
        .expect("session b load");
        assert_eq!(
            result.recovered, "UNSAVED session A's content",
            "test setup: session b must have inherited a's bridged draft"
        );

        reap_dead_sessions(&mut conn, &|_pid, _started_at| false, None).expect("reap");

        let recovered = crate::snapshot::recover_document(&conn, session_b, doc_id)
            .expect("recover_document must still succeed after the reap");
        assert_eq!(
            recovered, "UNSAVED session A's content",
            "the bridged draft must survive reaping the dead session it came from"
        );
    }

    #[test]
    fn reaper_spares_the_sessions_row_of_a_dead_session_holding_only_a_merges_row() {
        let mut conn = open();
        let session_old = seed_session(&conn, 111);
        let session_new = seed_session(&conn, 222);
        let doc_id = seed_doc(&conn);
        journal_one_edit(&mut conn, session_old, doc_id);
        journal_one_edit(&mut conn, session_new, doc_id);
        seed_merges_row(&conn, session_old, session_new, doc_id);

        reap_dead_sessions(&mut conn, &|pid, _| pid != 111, None).expect("reap");

        let old_events: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM events WHERE session_id=?1",
                params![session_old],
                |r| r.get(0),
            )
            .expect("count");
        assert_eq!(
            old_events, 0,
            "a dead session's footprint must still be reaped even when it holds a merges row"
        );
        assert!(
            sessions_row_exists(&conn, session_old),
            "a sessions row a merges row still references must survive as provenance"
        );
    }

    fn seed_session_at(
        conn: &Connection,
        pid: i64,
        proc_started_at: &str,
        opened_at: &str,
    ) -> SessionId {
        conn.execute(
            "INSERT INTO sessions(pid, proc_started_at, opened_at) VALUES(?1, ?2, ?3)",
            params![pid, proc_started_at, opened_at],
        )
        .expect("seed session");
        SessionId(conn.last_insert_rowid())
    }

    /// A legacy row that never captured a real `proc_started_at` (the
    /// liveness hole `establish_session` no longer produces) is confirmed
    /// dead once its own `opened_at` predates the injected boot instant —
    /// no process survives a reboot, so this must reap even when `is_alive`
    /// wrongly reports true for it.
    #[test]
    fn legacy_empty_started_at_before_boot_is_reaped_even_when_reported_alive() {
        let mut conn = open();
        let own_pid = std::process::id() as i64;
        let boot = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(2 * 24 * 3600);
        let before_boot = crate::session::format_rfc3339_nanos(
            SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(24 * 3600),
        );
        let session_old = seed_session_at(&conn, own_pid, "", &before_boot);
        let doc_id = seed_doc(&conn);
        journal_one_edit(&mut conn, session_old, doc_id);

        let session_new = seed_session(&conn, own_pid + 1);
        journal_one_edit(&mut conn, session_new, doc_id);

        reap_dead_sessions(&mut conn, &|_pid, _started_at| true, Some(boot)).expect("reap");

        let old_events: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM events WHERE session_id=?1",
                params![session_old],
                |r| r.get(0),
            )
            .expect("count");
        assert_eq!(
            old_events, 0,
            "a legacy '' row predating boot must be reaped despite is_alive reporting true"
        );
    }

    /// A legacy `proc_started_at=''` row whose `opened_at` is AFTER the
    /// injected boot instant is not a reboot-death candidate at all —
    /// ordinary `is_alive` liveness still governs it, and here it reports
    /// alive, so it must be spared.
    #[test]
    fn legacy_empty_started_at_after_boot_is_spared() {
        let mut conn = open();
        let own_pid = std::process::id() as i64;
        let boot = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(24 * 3600);
        let after_boot = crate::session::format_rfc3339_nanos(
            SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(2 * 24 * 3600),
        );
        let session_old = seed_session_at(&conn, own_pid, "", &after_boot);
        let doc_id = seed_doc(&conn);
        journal_one_edit(&mut conn, session_old, doc_id);

        let session_new = seed_session(&conn, own_pid + 1);
        journal_one_edit(&mut conn, session_new, doc_id);

        reap_dead_sessions(&mut conn, &|_pid, _started_at| true, Some(boot)).expect("reap");

        let old_events: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM events WHERE session_id=?1",
                params![session_old],
                |r| r.get(0),
            )
            .expect("count");
        assert_eq!(
            old_events, 1,
            "a legacy '' row after boot must stay governed by is_alive, not reboot-death"
        );
    }
}
