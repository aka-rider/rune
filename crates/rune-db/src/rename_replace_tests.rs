#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use rune_vfs::{Mem, OpKind as VfsOp};
use rusqlite::{Connection, params};

use super::*;
use crate::confirmation::Confirmation;
use crate::ids::DocId;
use crate::materialize::DocSession;
use crate::obs_origin::ObsOrigin;
use crate::observation::{self, ObservationMeta, StatFacts};
use crate::retry;
use crate::test_support::open;
use std::io;
use std::path::Path;
use std::time::SystemTime;

fn seed_doc_with_path(conn: &Connection, path: &str) -> DocId {
    conn.execute(
        "INSERT INTO documents(path, created_at, last_seen_at) VALUES (?1, 'x', 'x')",
        params![path],
    )
    .expect("seed doc");
    DocId(conn.last_insert_rowid())
}

fn publish(vfs: &Mem, path: &Path, bytes: &[u8]) {
    let temp = vfs.write_durable(path, bytes).expect("write_durable");
    vfs.rename_excl(&temp, path).expect("publish");
}

fn obs_count(conn: &Connection) -> i64 {
    conn.query_row("SELECT COUNT(*) FROM observations", [], |r| r.get(0))
        .expect("count observations")
}

fn doc_path(conn: &Connection, doc_id: DocId) -> String {
    conn.query_row(
        "SELECT path FROM documents WHERE id=?1",
        params![doc_id],
        |r| r.get(0),
    )
    .expect("doc path")
}

struct Fixture {
    conn: Connection,
    vfs: Mem,
    ds: DocSession,
}

fn fixture(content: &[u8]) -> Fixture {
    let conn = open();
    let vfs = Mem::new();
    let session_id =
        crate::session::establish_session(&conn, SystemTime::now()).expect("establish session");
    publish(&vfs, Path::new("/a.md"), content);
    let doc_id = seed_doc_with_path(&conn, "/a.md");
    Fixture {
        conn,
        vfs,
        ds: DocSession { doc_id, session_id },
    }
}

#[test]
fn rename_replace_preserves_the_displaced_bytes_as_a_durable_blob() {
    let mut f = fixture(b"ours");
    publish(&f.vfs, Path::new("/b.md"), b"theirs");
    let seen = f.vfs.stat(Path::new("/b.md")).expect("stat b");

    let out = rename_replace(
        &mut f.conn,
        &f.vfs,
        f.ds,
        Path::new("/a.md"),
        Path::new("/b.md"),
        seen,
        SystemTime::now(),
    )
    .expect("rename_replace");

    let RenameOutcome::Replaced { displaced, .. } = out else {
        panic!("expected Replaced, got {out:?}");
    };
    assert_eq!(displaced.doc_id, f.ds.doc_id, "captured under OUR doc");
    assert_eq!(displaced.origin, ObsOrigin::Swap);
    assert_eq!(
        displaced.blob_hash.as_str(),
        observation::hash_bytes(b"theirs"),
        "the blob must be the REPLACED file's bytes"
    );

    let blob = retry::with_retry(&mut f.conn, |tx| {
        crate::blob::get_blob(tx, displaced.blob_hash.as_str())
    })
    .expect("displaced bytes durably stored");
    assert_eq!(blob, b"theirs");

    assert_eq!(f.vfs.read(Path::new("/b.md")).expect("read to"), b"ours");
    assert!(f.vfs.read(Path::new("/a.md")).is_err(), "from removed");
    assert_eq!(doc_path(&f.conn, f.ds.doc_id), "/b.md");
}

#[test]
fn rename_replace_captures_non_utf8_displaced_bytes_byte_exact() {
    let mut f = fixture(b"ours");
    let theirs: &[u8] = &[0xff, 0xfe, 0x00, 0x9f, 0x92, 0x96, 0x80];
    publish(&f.vfs, Path::new("/b.md"), theirs);
    let seen = f.vfs.stat(Path::new("/b.md")).expect("stat b");

    let out = rename_replace(
        &mut f.conn,
        &f.vfs,
        f.ds,
        Path::new("/a.md"),
        Path::new("/b.md"),
        seen,
        SystemTime::now(),
    )
    .expect("non-utf8 displaced bytes must not hard-error");

    let RenameOutcome::Replaced { displaced, .. } = out else {
        panic!("expected Replaced, got {out:?}");
    };
    let blob = retry::with_retry(&mut f.conn, |tx| {
        crate::blob::get_blob(tx, displaced.blob_hash.as_str())
    })
    .expect("blob");
    assert_eq!(blob, theirs, "displaced bytes must round-trip byte-exact");
}

#[test]
fn rename_replace_refuses_when_the_destination_changed_since_consent() {
    let mut f = fixture(b"ours");
    publish(&f.vfs, Path::new("/b.md"), b"theirs");
    let seen = f.vfs.stat(Path::new("/b.md")).expect("stat b");
    let obs_before = obs_count(&f.conn);

    let temp = f
        .vfs
        .write_durable(Path::new("/b.md"), b"changed underneath")
        .expect("racer write");
    f.vfs
        .exchange(&temp, Path::new("/b.md"))
        .expect("racer exchange");
    f.vfs.remove(&temp).ok();

    let out = rename_replace(
        &mut f.conn,
        &f.vfs,
        f.ds,
        Path::new("/a.md"),
        Path::new("/b.md"),
        seen,
        SystemTime::now(),
    )
    .expect("a refusal is not an error");

    assert!(matches!(out, RenameOutcome::Refused { .. }));
    assert_eq!(f.vfs.read(Path::new("/a.md")).expect("a intact"), b"ours");
    assert_eq!(
        f.vfs.read(Path::new("/b.md")).expect("b intact"),
        b"changed underneath"
    );
    assert_eq!(doc_path(&f.conn, f.ds.doc_id), "/a.md");
    assert_eq!(obs_count(&f.conn), obs_before, "no blob, no rebind");
}

#[test]
fn rename_replace_exchange_failure_leaves_both_files_intact() {
    let mut f = fixture(b"ours");
    publish(&f.vfs, Path::new("/b.md"), b"theirs");
    let seen = f.vfs.stat(Path::new("/b.md")).expect("stat b");
    let obs_before = obs_count(&f.conn);
    f.vfs.fail_next(VfsOp::Exchange, io::ErrorKind::Other);

    let err = rename_replace(
        &mut f.conn,
        &f.vfs,
        f.ds,
        Path::new("/a.md"),
        Path::new("/b.md"),
        seen,
        SystemTime::now(),
    )
    .expect_err("exchange failure must surface");
    assert!(matches!(err, Error::Io(_)));

    assert_eq!(f.vfs.read(Path::new("/a.md")).expect("a intact"), b"ours");
    assert_eq!(f.vfs.read(Path::new("/b.md")).expect("b intact"), b"theirs");
    assert_eq!(doc_path(&f.conn, f.ds.doc_id), "/a.md");
    assert_eq!(obs_count(&f.conn), obs_before);
}

#[test]
fn rename_replace_post_swap_read_failure_leaves_the_displaced_bytes_at_from() {
    let mut f = fixture(b"ours");
    publish(&f.vfs, Path::new("/b.md"), b"theirs");
    let seen = f.vfs.stat(Path::new("/b.md")).expect("stat b");
    f.vfs.fail_next(VfsOp::Read, io::ErrorKind::Other);

    let err = rename_replace(
        &mut f.conn,
        &f.vfs,
        f.ds,
        Path::new("/a.md"),
        Path::new("/b.md"),
        seen,
        SystemTime::now(),
    )
    .expect_err("post-swap read failure must surface");
    let msg = err.to_string();
    assert!(
        msg.contains("/a.md"),
        "the error must name where the displaced bytes still are: {msg}"
    );

    assert_eq!(
        f.vfs
            .read(Path::new("/a.md"))
            .expect("displaced bytes kept"),
        b"theirs",
        "the replaced file's bytes must still be on disk at `from`"
    );
    assert_eq!(f.vfs.read(Path::new("/b.md")).expect("b"), b"ours");
}

#[test]
fn rename_replace_reports_success_with_durable_false_on_an_unconfirmed_exchange() {
    let mut f = fixture(b"ours");
    publish(&f.vfs, Path::new("/b.md"), b"theirs");
    let seen = f.vfs.stat(Path::new("/b.md")).expect("stat b");
    f.vfs.fail_after(VfsOp::Exchange, io::ErrorKind::Other);

    let out = rename_replace(
        &mut f.conn,
        &f.vfs,
        f.ds,
        Path::new("/a.md"),
        Path::new("/b.md"),
        seen,
        SystemTime::now(),
    )
    .expect("an unconfirmed-durability publish must not fail the replace");

    let RenameOutcome::Replaced { displaced, durable } = out else {
        panic!("expected Replaced, got {out:?}");
    };
    assert!(
        !durable,
        "unconfirmed exchange durability must report false"
    );

    let blob = retry::with_retry(&mut f.conn, |tx| {
        crate::blob::get_blob(tx, displaced.blob_hash.as_str())
    })
    .expect("displaced bytes durably stored despite unconfirmed exchange durability");
    assert_eq!(blob, b"theirs");

    assert_eq!(f.vfs.read(Path::new("/b.md")).expect("read to"), b"ours");
    assert_eq!(
        f.vfs.read(Path::new("/a.md")).expect("from kept"),
        b"theirs",
        "from must not be removed when the swap's own durability is unconfirmed"
    );
    assert_eq!(doc_path(&f.conn, f.ds.doc_id), "/b.md");
}

#[test]
fn rename_replace_remove_failure_still_reports_replaced_with_the_blob_committed() {
    let mut f = fixture(b"ours");
    publish(&f.vfs, Path::new("/b.md"), b"theirs");
    let seen = f.vfs.stat(Path::new("/b.md")).expect("stat b");
    f.vfs.fail_next(VfsOp::Remove, io::ErrorKind::Other);

    let out = rename_replace(
        &mut f.conn,
        &f.vfs,
        f.ds,
        Path::new("/a.md"),
        Path::new("/b.md"),
        seen,
        SystemTime::now(),
    )
    .expect("a failed unlink must not fail the replace");

    let RenameOutcome::Replaced { displaced, .. } = out else {
        panic!("expected Replaced, got {out:?}");
    };
    let blob = retry::with_retry(&mut f.conn, |tx| {
        crate::blob::get_blob(tx, displaced.blob_hash.as_str())
    })
    .expect("blob committed");
    assert_eq!(blob, b"theirs");
    assert_eq!(doc_path(&f.conn, f.ds.doc_id), "/b.md");
    assert_eq!(
        f.vfs.read(Path::new("/a.md")).expect("leftover"),
        b"theirs",
        "a leftover at `from` is recoverable clutter, never data loss"
    );
}

#[test]
fn rename_replace_blanks_the_displaced_documents_row_path() {
    let mut f = fixture(b"ours");
    publish(&f.vfs, Path::new("/b.md"), b"theirs");
    let other = seed_doc_with_path(&f.conn, "/b.md");
    let seen = f.vfs.stat(Path::new("/b.md")).expect("stat b");

    rename_replace(
        &mut f.conn,
        &f.vfs,
        f.ds,
        Path::new("/a.md"),
        Path::new("/b.md"),
        seen,
        SystemTime::now(),
    )
    .expect("rename_replace");

    assert_eq!(doc_path(&f.conn, f.ds.doc_id), "/b.md");
    assert_eq!(
        doc_path(&f.conn, other),
        "",
        "the displaced document's row must no longer claim /b.md"
    );
}

#[test]
fn rename_replace_over_a_bound_document_leaves_the_displaced_documents_ancestor_unchanged() {
    let mut f = fixture(b"ours");
    publish(&f.vfs, Path::new("/b.md"), b"theirs");
    let other = seed_doc_with_path(&f.conn, "/b.md");
    let other_session = crate::session::establish_session(&f.conn, SystemTime::now())
        .expect("establish other session");

    retry::with_retry(&mut f.conn, |tx| {
        let blob_hash = crate::blob::put_blob(tx, b"theirs")?;
        observation::record_observation(
            tx,
            other,
            other_session,
            ObservationMeta {
                blob_hash: &blob_hash,
                seq: Some(0),
                origin: ObsOrigin::Load,
                confirmed: Confirmation::Unclassified,
            },
            &StatFacts {
                size: Some(6),
                mtime: Some("t".to_string()),
                ..Default::default()
            },
            "t",
        )
    })
    .expect("seed other's ancestor observation");

    let before = retry::with_retry(&mut f.conn, |tx| {
        observation::ancestor_at(tx, other, other_session, 0, None)
    })
    .expect("ancestor before")
    .expect("other must have an ancestor before the replace");

    let seen = f.vfs.stat(Path::new("/b.md")).expect("stat b");
    rename_replace(
        &mut f.conn,
        &f.vfs,
        f.ds,
        Path::new("/a.md"),
        Path::new("/b.md"),
        seen,
        SystemTime::now(),
    )
    .expect("rename_replace");

    let after = retry::with_retry(&mut f.conn, |tx| {
        observation::ancestor_at(tx, other, other_session, 0, None)
    })
    .expect("ancestor after")
    .expect("other must still have an ancestor after the replace");

    assert_eq!(
        after, before,
        "the displaced document's merge ancestor must be byte-identical \
         after a rename_replace swapped its path away"
    );
}
