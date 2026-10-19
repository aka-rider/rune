use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::Error;

pub fn set(tx: &Transaction<'_>, key: &str, value: &str) -> Result<(), Error> {
    tx.execute(
        "INSERT INTO ui_settings(key, value) VALUES(?1, ?2) \
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![key, value],
    )?;
    Ok(())
}

pub fn get(conn: &Connection, key: &str) -> Result<Option<String>, Error> {
    Ok(conn
        .query_row(
            "SELECT value FROM ui_settings WHERE key=?1",
            params![key],
            |r| r.get::<_, String>(0),
        )
        .optional()?)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::test_support::open;

    fn set_committed(conn: &mut Connection, key: &str, value: &str) {
        let tx = conn.transaction().expect("tx");
        set(&tx, key, value).expect("set");
        tx.commit().expect("commit");
    }

    #[test]
    fn an_unset_key_reads_none_and_a_set_key_reads_its_latest_value() {
        let mut conn = open();
        assert_eq!(get(&conn, "k").unwrap(), None);

        set_committed(&mut conn, "k", "one");
        set_committed(&mut conn, "k", "two");
        set_committed(&mut conn, "other", "x");

        assert_eq!(get(&conn, "k").unwrap().as_deref(), Some("two"));
    }
}
