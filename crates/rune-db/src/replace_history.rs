use rusqlite::{Connection, Transaction, params};

use crate::Error;

pub fn touch(tx: &Transaction<'_>, text: &str, now: std::time::SystemTime) -> Result<(), Error> {
    if text.trim().is_empty() {
        return Ok(());
    }
    let at = crate::session::format_rfc3339_nanos(now);
    tx.execute(
        "INSERT INTO replace_history(text, last_used_at) VALUES(?1, ?2) \
         ON CONFLICT(text) DO UPDATE SET last_used_at=excluded.last_used_at",
        params![text, at],
    )?;
    Ok(())
}

pub fn recent(conn: &Connection, limit: u32) -> Result<Vec<String>, Error> {
    let mut stmt =
        conn.prepare("SELECT text FROM replace_history ORDER BY last_used_at DESC LIMIT ?1")?;
    let rows = stmt.query_map(params![limit], |r| r.get::<_, String>(0))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use std::time::{Duration, SystemTime};

    use super::*;
    use crate::test_support::open;

    #[test]
    fn touching_the_same_text_twice_bumps_last_used_at_without_duplicating() {
        let mut conn = open();
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1000);
        let t1 = t0 + Duration::from_secs(60);

        for at in [t0, t1] {
            let tx = conn.transaction().expect("tx");
            touch(&tx, "cat", at).expect("touch");
            tx.commit().expect("commit");
        }

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM replace_history", [], |r| r.get(0))
            .expect("count");
        assert_eq!(count, 1, "must upsert, never duplicate");

        let last_used_at: String = conn
            .query_row(
                "SELECT last_used_at FROM replace_history WHERE text = 'cat'",
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
        for (i, text) in ["alpha", "beta", "gamma"].into_iter().enumerate() {
            let tx = conn.transaction().expect("tx");
            touch(&tx, text, base + Duration::from_secs(i as u64 * 10)).expect("touch");
            tx.commit().expect("commit");
        }

        assert_eq!(
            recent(&conn, 10).expect("recent all"),
            vec!["gamma", "beta", "alpha"]
        );
        assert_eq!(
            recent(&conn, 2).expect("recent limited"),
            vec!["gamma", "beta"]
        );
    }

    #[test]
    fn empty_or_whitespace_text_is_not_persisted() {
        let mut conn = open();
        {
            let tx = conn.transaction().expect("tx");
            touch(&tx, "", SystemTime::UNIX_EPOCH).expect("touch empty");
            touch(&tx, "   ", SystemTime::UNIX_EPOCH).expect("touch whitespace");
            tx.commit().expect("commit");
        }

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM replace_history", [], |r| r.get(0))
            .expect("count");
        assert_eq!(count, 0);
    }

    #[test]
    fn find_and_replace_histories_never_share_rows() {
        let mut conn = open();
        {
            let tx = conn.transaction().expect("tx");
            crate::search_history::touch(&tx, "dog", SystemTime::UNIX_EPOCH).expect("touch find");
            touch(&tx, "cat", SystemTime::UNIX_EPOCH).expect("touch replace");
            tx.commit().expect("commit");
        }

        assert_eq!(recent(&conn, 10).expect("replace recents"), vec!["cat"]);
        assert_eq!(
            crate::search_history::recent(&conn, 10).expect("find recents"),
            vec!["dog"]
        );
    }
}
