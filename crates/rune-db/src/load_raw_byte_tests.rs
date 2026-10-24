#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::*;
use crate::inherit::Inherited;
use crate::journal_append::EditBatch;
use crate::test_support::{always_dead, open};
use rune_core::buffer::AppliedEdit;
use rune_core::undo::EditKind;
use rune_vfs::Mem;

const LATIN1: &[u8] = b"caf\xe9\n";

fn publish(vfs: &Mem, path: &Path, bytes: &[u8]) {
    let temp = vfs.write_durable(path, bytes).expect("write_durable");
    vfs.rename_excl(&temp, path).expect("publish");
}

fn load_as(
    conn: &mut Connection,
    vfs: &Mem,
    session_id: SessionId,
    path: &Path,
) -> Result<LoadResult, Error> {
    load(conn, vfs, session_id, &always_dead, path, SystemTime::now())
}

fn new_session(conn: &Connection) -> SessionId {
    crate::session::establish_session(conn, SystemTime::now()).expect("session")
}

fn snapshot_hashes(conn: &Connection, doc_id: DocId) -> Vec<String> {
    let mut stmt = conn
        .prepare("SELECT blob_hash FROM snapshots WHERE doc_id=?1")
        .expect("prepare");
    stmt.query_map(params![doc_id], |r| r.get(0))
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("rows")
}

fn observation_hash(conn: &Connection, obs: ObsId) -> String {
    conn.query_row(
        "SELECT blob_hash FROM observations WHERE id=?1",
        params![obs],
        |r| r.get(0),
    )
    .expect("observation hash")
}

fn replace_at_start(
    conn: &mut Connection,
    session_id: SessionId,
    doc_id: DocId,
    deleted: &str,
    insert: &str,
) {
    let tx = conn.transaction().expect("tx");
    crate::journal::append_edit(
        &tx,
        session_id,
        SystemTime::now(),
        doc_id,
        EditBatch {
            edits: &[AppliedEdit {
                start: 0,
                end: deleted.len(),
                deleted: deleted.to_string(),
                insert: insert.to_string(),
            }],
            cursors_before: &[],
            cursors_after: &[],
            kind: EditKind::Other,
        },
    )
    .expect("append_edit");
    tx.commit().expect("commit");
}

fn type_at_start(conn: &mut Connection, session_id: SessionId, doc_id: DocId, insert: &str) {
    replace_at_start(conn, session_id, doc_id, "", insert);
}

#[test]
fn a_non_utf8_file_loads_and_its_anchor_snapshot_dedupes_with_the_observation() {
    let mut conn = open();
    let vfs = Mem::new();
    let session = new_session(&conn);
    let path = Path::new("/latin1.md");
    publish(&vfs, path, LATIN1);

    let result = load_as(&mut conn, &vfs, session, path).expect("load accepts any bytes");

    assert_eq!(&*codec::encode(&result.disk_content), LATIN1);
    assert_eq!(result.recovered.content, result.disk_content);
    let recovered =
        crate::snapshot::recover_document(&conn.transaction().expect("tx"), session, result.doc_id)
            .expect("recover");
    assert_eq!(&*codec::encode(&recovered.content), LATIN1);

    let disk_hash = observation::hash_bytes(LATIN1);
    assert_eq!(snapshot_hashes(&conn, result.doc_id), [disk_hash.as_str()]);
    let saved = result.saved_obs.expect("first load adopts");
    assert_eq!(observation_hash(&conn, saved), disk_hash);
    assert_eq!(
        crate::blob::get_blob(&conn, &disk_hash).expect("blob"),
        LATIN1
    );
}

#[test]
fn reloading_an_unedited_non_utf8_file_is_clean() {
    let mut conn = open();
    let vfs = Mem::new();
    let session = new_session(&conn);
    let path = Path::new("/latin1.md");
    publish(&vfs, path, LATIN1);
    load_as(&mut conn, &vfs, session, path).expect("first load");

    let again = load_as(&mut conn, &vfs, session, path).expect("reload");

    assert!(again.has_history);
    assert_eq!(again.sync.kind, crate::sync::SyncKind::Clean);
    assert_eq!(&*codec::encode(&again.recovered.content), LATIN1);
}

#[test]
fn a_dead_sessions_unchanged_non_utf8_doc_inherits_nothing() {
    let mut conn = open();
    let vfs = Mem::new();
    let session = new_session(&conn);
    let path = Path::new("/latin1.md");
    publish(&vfs, path, LATIN1);
    let doc_id = load_as(&mut conn, &vfs, session, path)
        .expect("load")
        .doc_id;

    let inherited = find_inheritable_draft(
        &mut conn,
        &always_dead,
        doc_id,
        &observation::hash_bytes(LATIN1),
    )
    .expect("find_inheritable_draft");

    assert!(matches!(inherited, Inherited::Disk));
}

#[test]
fn a_dead_sessions_edit_of_a_non_utf8_doc_is_inherited_byte_exact() {
    let mut conn = open();
    let vfs = Mem::new();
    let dead = new_session(&conn);
    let path = Path::new("/latin1.md");
    publish(&vfs, path, LATIN1);
    let doc_id = load_as(&mut conn, &vfs, dead, path).expect("load").doc_id;
    type_at_start(&mut conn, dead, doc_id, "X");

    let heir = new_session(&conn);
    let result = load_as(&mut conn, &vfs, heir, path).expect("heir load");

    assert_eq!(&*codec::encode(&result.recovered.content), b"Xcaf\xe9\n");
    assert_eq!(&*codec::encode(&result.disk_content), LATIN1);
}

fn dead_session_with_edits(
    conn: &mut Connection,
    vfs: &Mem,
    path: &Path,
    edits: &[(&str, &str)],
) -> DocId {
    let dead = new_session(conn);
    publish(vfs, path, LATIN1);
    let doc_id = load_as(conn, vfs, dead, path).expect("load").doc_id;
    for (deleted, insert) in edits {
        replace_at_start(conn, dead, doc_id, deleted, insert);
    }
    doc_id
}

#[test]
fn a_dead_sessions_draft_that_encodes_to_the_current_disk_bytes_inherits_nothing() {
    let mut conn = open();
    let vfs = Mem::new();
    let doc_id = dead_session_with_edits(&mut conn, &vfs, Path::new("/latin1.md"), &[("", "X")]);

    let inherited = find_inheritable_draft(
        &mut conn,
        &always_dead,
        doc_id,
        &observation::hash_bytes(b"Xcaf\xe9\n"),
    )
    .expect("find_inheritable_draft");

    assert!(matches!(inherited, Inherited::Disk));
}

#[test]
fn a_dead_sessions_draft_that_encodes_to_its_own_baseline_inherits_nothing() {
    let mut conn = open();
    let vfs = Mem::new();
    let doc_id = dead_session_with_edits(
        &mut conn,
        &vfs,
        Path::new("/latin1.md"),
        &[("", "X"), ("X", "")],
    );

    let inherited = find_inheritable_draft(
        &mut conn,
        &always_dead,
        doc_id,
        &observation::hash_bytes(b"moved on\n"),
    )
    .expect("find_inheritable_draft");

    assert!(matches!(inherited, Inherited::Disk));
}
