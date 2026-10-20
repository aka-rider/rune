#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::*;

const V2_ORIGINAL_TABLES: [&str; 10] = [
    "documents",
    "blobs",
    "sessions",
    "session_documents",
    "snapshots",
    "events",
    "observations",
    "merges",
    "search_history",
    "command_history",
];

fn tables_added_in_place(conn: &Connection) -> Vec<String> {
    let mut stmt = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'")
        .expect("prepare table list");
    stmt.query_map([], |row| row.get::<_, String>(0))
        .expect("list tables")
        .map(|name| name.expect("table name"))
        .filter(|name| !V2_ORIGINAL_TABLES.contains(&name.as_str()))
        .collect()
}

fn delete_actions(conn: &Connection, table: &str) -> Vec<(String, String)> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA foreign_key_list({table})"))
        .expect("prepare foreign_key_list");
    stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>("table")?,
            row.get::<_, String>("on_delete")?,
        ))
    })
    .expect("read foreign_key_list")
    .map(|fk| fk.expect("foreign key row"))
    .collect()
}

#[test]
fn a_table_added_in_place_never_blocks_an_older_binarys_delete() {
    let mut conn = Connection::open_in_memory().expect("open");
    apply(&mut conn).expect("apply");

    let added = tables_added_in_place(&conn);
    assert!(
        added.iter().any(|table| table == "doc_search_history"),
        "the walk must reach the in-place tables: {added:?}"
    );
    let mut checked = 0;
    for table in &added {
        for (parent, action) in delete_actions(&conn, table) {
            checked += 1;
            assert!(
                action == "CASCADE" || action == "SET NULL",
                "{table} references {parent} with ON DELETE {action}; an older binary deleting \
                 from {parent} would fail or strand rows"
            );
        }
    }
    assert!(checked > 0, "no foreign key was inspected: {added:?}");
}

#[test]
fn delete_actions_reports_a_restricting_reference() {
    let conn = Connection::open_in_memory().expect("open");
    conn.execute_batch(
        "CREATE TABLE parent(id INTEGER PRIMARY KEY);
         CREATE TABLE child(p INTEGER REFERENCES parent(id));",
    )
    .expect("create tables");

    assert_eq!(
        delete_actions(&conn, "child"),
        vec![("parent".to_string(), "NO ACTION".to_string())]
    );
}
