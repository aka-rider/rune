use rusqlite::{Connection, Transaction, params};

use crate::Error;
use crate::ids::DocId;

pub fn touch(
    tx: &Transaction<'_>,
    doc_id: DocId,
    query: &str,
    now: std::time::SystemTime,
) -> Result<(), Error> {
    if query.trim().is_empty() {
        return Ok(());
    }
    let at = crate::session::format_rfc3339_nanos(now);
    tx.execute(
        "INSERT INTO doc_search_history(doc_id, query, last_used_at) VALUES(?1, ?2, ?3) \
         ON CONFLICT(doc_id, query) DO UPDATE SET last_used_at=excluded.last_used_at",
        params![doc_id, query, at],
    )?;
    Ok(())
}

pub fn recent(conn: &Connection, doc_id: DocId, limit: u32) -> Result<Vec<String>, Error> {
    let mut stmt = conn.prepare(
        "SELECT query FROM doc_search_history WHERE doc_id=?1 \
         ORDER BY last_used_at DESC LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![doc_id, limit], |r| r.get::<_, String>(0))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use std::time::{Duration, SystemTime};

    use super::*;
    use crate::test_support::{always_dead, insert_test_document, open};

    fn touch_at(conn: &mut Connection, doc_id: DocId, query: &str, secs: u64) {
        let tx = conn.transaction().expect("tx");
        touch(
            &tx,
            doc_id,
            query,
            SystemTime::UNIX_EPOCH + Duration::from_secs(secs),
        )
        .expect("touch");
        tx.commit().expect("commit");
    }

    fn two_documents(conn: &mut Connection) -> (DocId, DocId) {
        let tx = conn.transaction().expect("tx");
        let ids = (insert_test_document(&tx), insert_test_document(&tx));
        tx.commit().expect("commit");
        ids
    }

    #[test]
    fn recent_is_mru_first_honors_limit_and_is_scoped_to_the_document() {
        let mut conn = open();
        let (a, b) = two_documents(&mut conn);
        touch_at(&mut conn, a, "alpha", 10);
        touch_at(&mut conn, a, "beta", 20);
        touch_at(&mut conn, b, "other", 30);
        touch_at(&mut conn, a, "alpha", 40);

        assert_eq!(recent(&conn, a, 10).unwrap(), vec!["alpha", "beta"]);
        assert_eq!(recent(&conn, a, 1).unwrap(), vec!["alpha"]);
        assert_eq!(recent(&conn, b, 10).unwrap(), vec!["other"]);
    }

    #[test]
    fn empty_or_whitespace_query_is_not_persisted() {
        let mut conn = open();
        let (a, _) = two_documents(&mut conn);
        touch_at(&mut conn, a, "", 1);
        touch_at(&mut conn, a, "   ", 2);

        assert!(recent(&conn, a, 10).unwrap().is_empty());
    }

    #[test]
    fn touching_a_document_that_does_not_exist_is_an_error() {
        let mut conn = open();
        let tx = conn.transaction().expect("tx");
        let err = touch(&tx, DocId(999), "q", SystemTime::UNIX_EPOCH);
        assert!(err.is_err(), "the foreign key must reject an unknown doc");
    }

    #[test]
    fn forgetting_a_draft_deletes_its_history_and_keeps_the_others() {
        let mut conn = open();
        let session = crate::session::establish_session(&conn, SystemTime::now()).expect("session");
        let draft =
            crate::scratch::create_scratch_with_intent(&mut conn, session, SystemTime::now(), None)
                .expect("draft");
        let keep = {
            let tx = conn.transaction().expect("tx");
            let id = insert_test_document(&tx);
            tx.commit().expect("commit");
            id
        };
        touch_at(&mut conn, draft, "gone", 1);
        touch_at(&mut conn, keep, "stays", 2);

        let outcome = crate::scratch::forget_scratch(&mut conn, session, draft, &always_dead)
            .expect("forget");
        assert_eq!(outcome, crate::scratch::ForgetOutcome::Forgotten);

        assert!(recent(&conn, draft, 10).unwrap().is_empty());
        assert_eq!(recent(&conn, keep, 10).unwrap(), vec!["stays"]);
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM doc_search_history", [], |r| r.get(0))
            .expect("count");
        assert_eq!(rows, 1, "the forgotten draft's rows are physically gone");
    }
}
