//! Append-storm and racing scenarios.

use rusqlite::params;

use crate::support::{
    MARKER_SAFETY_DEADLINE, seed_schema_and_docs, spawn_helper, temp_dir, touch,
    wait_ready_or_child_death,
};

#[test]
fn four_children_append_storm_one_doc_each_all_ack_ok_with_exact_event_counts() {
    let dir = temp_dir("append-storm");
    let path = dir.join("rune-v1.db");
    let doc_ids = seed_schema_and_docs(&path, 4);
    let count = 25usize;
    let go = dir.join("go");

    let mut children = Vec::new();
    let mut readies = Vec::new();
    for (i, doc_id) in doc_ids.iter().enumerate() {
        let ready = dir.join(format!("ready-{i}"));
        readies.push(ready.clone());
        children.push(spawn_helper(
            "append_storm",
            &[
                ("RUNE_DB_PATH", path.display().to_string()),
                ("RUNE_DB_DOC_ID", doc_id.to_string()),
                ("RUNE_DB_COUNT", count.to_string()),
                ("RUNE_DB_READY_MARKER", ready.display().to_string()),
                ("RUNE_DB_GO_MARKER", go.display().to_string()),
            ],
        ));
    }

    wait_ready_or_child_death(&mut children, &readies, MARKER_SAFETY_DEADLINE);
    touch(&go);

    for child in children {
        let output = child.wait_with_output().expect("wait child");
        assert!(
            output.status.success(),
            "append_storm child failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let verify =
        rune_db::open_raw_connection_at_path_for_test(&path).expect("open verify connection");
    for doc_id in &doc_ids {
        let n: i64 = verify
            .query_row(
                "SELECT COUNT(*) FROM events WHERE doc_id=?1",
                params![doc_id],
                |r| r.get(0),
            )
            .expect("count events");
        assert_eq!(
            n, count as i64,
            "doc {doc_id} must have exactly {count} events"
        );
    }
    let total: i64 = verify
        .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))
        .expect("count total events");
    assert_eq!(total, 4 * count as i64);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn two_children_race_store_open_on_a_fresh_path_apply_schema_once_both_get_sessions() {
    let dir = temp_dir("race-open");
    let path = dir.join("rune-v1.db");
    let go = dir.join("go");

    let mut children = Vec::new();
    let mut readies = Vec::new();
    let mut openeds = Vec::new();
    for i in 0..2 {
        let ready = dir.join(format!("ready-{i}"));
        let opened = dir.join(format!("opened-{i}"));
        readies.push(ready.clone());
        openeds.push(opened.clone());
        children.push(spawn_helper(
            "race_open",
            &[
                ("RUNE_DB_PATH", path.display().to_string()),
                ("RUNE_DB_READY_MARKER", ready.display().to_string()),
                ("RUNE_DB_GO_MARKER", go.display().to_string()),
                ("RUNE_DB_OPENED_MARKER", opened.display().to_string()),
            ],
        ));
    }

    wait_ready_or_child_death(&mut children, &readies, MARKER_SAFETY_DEADLINE);
    touch(&go);

    for child in children {
        let output = child.wait_with_output().expect("wait child");
        assert!(
            output.status.success(),
            "race_open child failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let verify =
        rune_db::open_raw_connection_at_path_for_test(&path).expect("open verify connection");
    let integrity: String = verify
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .expect("integrity check");
    assert_eq!(integrity, "ok");

    let session_ids: Vec<i64> = openeds
        .iter()
        .map(|p| {
            std::fs::read_to_string(p)
                .expect("read opened marker")
                .trim()
                .parse()
                .expect("parse opened session id")
        })
        .collect();
    assert_ne!(
        session_ids[0], session_ids[1],
        "both racing opens must each have established their own distinct session"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn two_stores_closing_simultaneously_surface_no_error_despite_truncate_contention() {
    let dir = temp_dir("race-close");
    let path = dir.join("rune-v1.db");
    let _ = seed_schema_and_docs(&path, 0);
    let go = dir.join("go");

    let mut children = Vec::new();
    let mut readies = Vec::new();
    for i in 0..2 {
        let ready = dir.join(format!("ready-{i}"));
        readies.push(ready.clone());
        children.push(spawn_helper(
            "race_close",
            &[
                ("RUNE_DB_PATH", path.display().to_string()),
                ("RUNE_DB_READY_MARKER", ready.display().to_string()),
                ("RUNE_DB_GO_MARKER", go.display().to_string()),
            ],
        ));
    }

    wait_ready_or_child_death(&mut children, &readies, MARKER_SAFETY_DEADLINE);
    touch(&go);

    for child in children {
        let output = child.wait_with_output().expect("wait child");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "race_close child failed: {stderr}");
        assert!(
            !stderr.contains("panicked"),
            "child stderr shows a panic despite BUSY-class TRUNCATE contention being \
             expected and swallowed by design: {stderr}"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}
