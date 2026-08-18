use rusqlite::{Connection, Transaction, params};

use crate::Error;

pub fn touch(tx: &Transaction<'_>, name: &str, now: std::time::SystemTime) -> Result<(), Error> {
    if name.trim().is_empty() {
        return Ok(());
    }
    let at = crate::session::format_rfc3339_nanos(now);
    tx.execute(
        "INSERT INTO command_history(name, last_used_at) VALUES(?1, ?2) \
         ON CONFLICT(name) DO UPDATE SET last_used_at=excluded.last_used_at",
        params![name, at],
    )?;
    Ok(())
}

pub fn recent(conn: &Connection, limit: u32) -> Result<Vec<String>, Error> {
    let mut stmt =
        conn.prepare("SELECT name FROM command_history ORDER BY last_used_at DESC LIMIT ?1")?;
    let rows = stmt.query_map(params![limit], |r| r.get::<_, String>(0))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use std::time::{Duration, SystemTime};

    use rusqlite::Connection;

    use super::*;

    fn open() -> Connection {
        crate::conn::open_recovery_store(crate::conn::RecoveryTarget::Memory(
            &crate::conn::memory_uri(),
        ))
        .expect("open")
    }

    #[test]
    fn touching_the_same_name_twice_bumps_last_used_at_without_duplicating() {
        let mut conn = open();
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1000);
        let t1 = t0 + Duration::from_secs(60);

        {
            let tx = conn.transaction().expect("tx");
            touch(&tx, "save", t0).expect("touch t0");
            tx.commit().expect("commit");
        }
        {
            let tx = conn.transaction().expect("tx");
            touch(&tx, "save", t1).expect("touch t1");
            tx.commit().expect("commit");
        }

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM command_history", [], |r| r.get(0))
            .expect("count");
        assert_eq!(count, 1, "must upsert, never duplicate");

        let last_used_at: String = conn
            .query_row(
                "SELECT last_used_at FROM command_history WHERE name = 'save'",
                [],
                |r| r.get(0),
            )
            .expect("read last_used_at");
        assert_eq!(last_used_at, crate::session::format_rfc3339_nanos(t1));
    }

    #[test]
    fn recent_returns_mru_first_honoring_limit() {
        let mut conn = open();
        let base = SystemTime::UNIX_EPOCH + Duration::from_secs(1000);
        for (i, name) in ["save", "open", "quit"].into_iter().enumerate() {
            let tx = conn.transaction().expect("tx");
            touch(&tx, name, base + Duration::from_secs(i as u64 * 10)).expect("touch");
            tx.commit().expect("commit");
        }

        let all = recent(&conn, 10).expect("recent all");
        assert_eq!(all, vec!["quit", "open", "save"]);

        let limited = recent(&conn, 2).expect("recent limited");
        assert_eq!(limited, vec!["quit", "open"]);
    }

    #[test]
    fn empty_or_whitespace_name_is_not_persisted() {
        let mut conn = open();
        {
            let tx = conn.transaction().expect("tx");
            touch(&tx, "", SystemTime::UNIX_EPOCH).expect("touch empty");
            touch(&tx, "   ", SystemTime::UNIX_EPOCH).expect("touch whitespace");
            tx.commit().expect("commit");
        }

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM command_history", [], |r| r.get(0))
            .expect("count");
        assert_eq!(count, 0);
    }
}
