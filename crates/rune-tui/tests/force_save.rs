//! Integration tests for the disk-conflict Guard's `[S]ave anyway` answer: a
//! force-save that bypasses the compare-and-swap entirely rather than
//! retrying it, plus the
//! baseline-lifecycle fix that keeps a save started right after a
//! lost-bookkeeping commit from conflicting against this session's own
//! bytes. Driven through `rune_fuzz::Session`, pulling shared setup from
//! `merge_common`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod merge_common;

use std::path::Path;

use rune_fuzz::Session;
use rune_tui::document::DocumentId;
use rune_tui::guard::GuardKind;

use merge_common::{ch, drain_materialize_round_trip, external_write, save_and_ack, sup};

/// Sets up a document whose disk changed since it was opened, edits the
/// buffer, and drives a real `⌘S` all the way through the materialize dance
/// to the point where `handle_materialize_ack` raises the disk-conflict
/// Guard — the same fixture `merge_disk_conflict_guard.rs` builds, needed
/// again here because it is private to that file.
fn enter_disk_conflict_guard(disk_bytes: &[u8]) -> (Session, DocumentId) {
    let mut session = Session::open("/doc.md", "hello");
    let doc_id = session.app().active;

    assert!(session.key(ch('!')).is_none());
    assert_eq!(
        session.app().doc(doc_id).unwrap().buffer.content(),
        "!hello"
    );
    assert!(session.deliver_db().is_none());

    external_write(session.app().vfs.as_ref(), disk_bytes);

    save_and_ack(&mut session);

    (session, doc_id)
}

/// The escape hatch's whole point: a force-save publishes over whatever is
/// actually on disk and keeps what it displaced — never refusing a second
/// time, and never discarding the bytes it overwrote.
#[test]
fn disk_conflict_guard_save_anyway_force_saves_and_preserves_the_displaced_bytes() {
    let (mut session, doc_id) = enter_disk_conflict_guard(b"disk changed underneath");
    let Some(prompt) = &session.app().guard else {
        panic!("expected the disk-conflict Guard");
    };
    assert!(matches!(prompt.kind, GuardKind::DiskConflict));

    assert!(session.key(ch('s')).is_none());
    assert!(
        session.app().guard.is_none(),
        "[S]ave anyway must clear the Guard immediately, not wait on the save's own ack"
    );

    drain_materialize_round_trip(&mut session);

    assert!(
        session.app().guard.is_none(),
        "a force-save must never re-raise the conflict it was answering"
    );
    assert_eq!(
        session.app().doc(doc_id).unwrap().buffer.content(),
        "!hello"
    );
    assert!(
        !session.app().doc(doc_id).unwrap().is_dirty(),
        "a committed force-save must clear the dirty flag"
    );
    assert_eq!(
        session.app().vfs.read(Path::new("/doc.md")).unwrap(),
        b"!hello",
        "the destination must hold the buffer's bytes, not the interloper's"
    );

    let hash = rune_db::hash_bytes(b"disk changed underneath");
    let blob = session
        .app()
        .db
        .as_ref()
        .unwrap()
        .store
        .reader()
        .query(rune_db::ReaderRequestKind::GetBlob { hash })
        .expect("the displaced bytes must be queryable from the store");
    assert_eq!(
        blob,
        rune_db::ReaderReply::Blob(b"disk changed underneath".to_vec()),
        "the interloper's exact bytes must be durably recoverable"
    );

    let log = rune_tui::messages::log_text(session.app());
    assert!(
        log.contains("preserved"),
        "a successful force-save that displaced foreign bytes must say so: {log:?}"
    );
}

/// Force-save truthfulness: when the disk moved back to the CAS baseline
/// before `[S]ave anyway` publishes, nothing foreign is displaced — the
/// save commits plainly and the "concurrent external change was
/// overwritten" message must NOT fire.
#[test]
fn disk_conflict_guard_save_anyway_over_the_restored_baseline_posts_no_race_message() {
    let (mut session, doc_id) = enter_disk_conflict_guard(b"disk changed underneath");

    // The interloper reverts its change while the Guard is up — the disk
    // holds the exact baseline bytes this session loaded.
    external_write(session.app().vfs.as_ref(), b"hello");

    assert!(session.key(ch('s')).is_none());
    drain_materialize_round_trip(&mut session);

    assert!(session.app().guard.is_none());
    assert!(!session.app().doc(doc_id).unwrap().is_dirty());
    assert_eq!(
        session.app().vfs.read(Path::new("/doc.md")).unwrap(),
        b"!hello"
    );
    let log = rune_tui::messages::log_text(session.app());
    assert!(
        !log.contains("preserved"),
        "a force-save that displaced only its own baseline overwrote nothing \
         foreign and must not claim it did: {log:?}"
    );
}

/// The actual bug this WP kills: a CAS *retry* refuses again the moment the
/// disk moves a second time, making the old `[S]ave anyway` useless exactly
/// when the user needed it most. A force-save must succeed regardless of how
/// many more times the disk moved between the conflict and the answer, and
/// must preserve whatever it most recently displaced — not a stale snapshot
/// from when the conflict was first detected.
#[test]
fn disk_conflict_guard_save_anyway_succeeds_even_if_the_disk_moves_again_before_the_answer() {
    let (mut session, doc_id) = enter_disk_conflict_guard(b"disk changed once");

    // The disk moves AGAIN while the Guard is still up — this is exactly
    // what made a CAS *retry* refuse a second time.
    external_write(session.app().vfs.as_ref(), b"disk changed twice");

    assert!(session.key(ch('s')).is_none());
    drain_materialize_round_trip(&mut session);

    assert!(
        session.app().guard.is_none(),
        "a force-save must never re-raise the conflict it was answering, \
         no matter how many times the disk moved in between"
    );
    assert_eq!(
        session.app().doc(doc_id).unwrap().buffer.content(),
        "!hello"
    );
    assert!(!session.app().doc(doc_id).unwrap().is_dirty());
    assert_eq!(
        session.app().vfs.read(Path::new("/doc.md")).unwrap(),
        b"!hello"
    );

    let hash = rune_db::hash_bytes(b"disk changed twice");
    let blob = session
        .app()
        .db
        .as_ref()
        .unwrap()
        .store
        .reader()
        .query(rune_db::ReaderRequestKind::GetBlob { hash })
        .expect("the LATEST displaced bytes must be queryable from the store");
    assert_eq!(
        blob,
        rune_db::ReaderReply::Blob(b"disk changed twice".to_vec()),
        "the force publish must capture what it actually displaced at \
         publish time, not the stale snapshot from conflict-detection time"
    );
}

/// The destination vanishing between the conflict and the retry (someone
/// deleted or renamed it out from under the Guard) must still succeed —
/// existence-aware publish falls back to a no-clobber create.
#[test]
fn disk_conflict_guard_save_anyway_recreates_the_file_when_it_vanished_meanwhile() {
    let (mut session, doc_id) = enter_disk_conflict_guard(b"disk changed underneath");

    session
        .app()
        .vfs
        .remove(Path::new("/doc.md"))
        .expect("remove the destination while the Guard is up");

    assert!(session.key(ch('s')).is_none());
    drain_materialize_round_trip(&mut session);

    assert!(session.app().guard.is_none());
    assert_eq!(
        session.app().doc(doc_id).unwrap().buffer.content(),
        "!hello"
    );
    assert!(!session.app().doc(doc_id).unwrap().is_dirty());
    assert_eq!(
        session.app().vfs.read(Path::new("/doc.md")).unwrap(),
        b"!hello"
    );
}

/// The ordinary compare-and-swap path is untouched by any of the above: once
/// the disk stops moving, a plain `⌘S` still succeeds without the user ever
/// having needed the force-save escape hatch.
#[test]
fn an_ordinary_save_still_succeeds_once_the_disk_is_quiet() {
    let mut session = Session::open("/doc.md", "hello");
    let doc_id = session.app().active;

    assert!(session.key(ch('!')).is_none());
    assert!(session.deliver_db().is_none());

    save_and_ack(&mut session);

    assert!(
        session.app().guard.is_none(),
        "a save against a quiet disk must never raise the conflict guard"
    );
    assert!(!session.app().doc(doc_id).unwrap().is_dirty());
    assert_eq!(
        session.app().vfs.read(Path::new("/doc.md")).unwrap(),
        b"!hello",
        "the ordinary CAS path must still publish correctly"
    );
}

/// The disk-conflict Guard's `[S]ave anyway` is the user's explicit
/// last-resort consent — a refused attempt (here, a save genuinely already
/// running for the same document, driven through a real `⌘S` whose own ack
/// is deliberately left undelivered) must never destroy the prompt that
/// consent was answering. A second `[S]` once that save has actually
/// finished then succeeds normally.
#[test]
fn disk_conflict_prompt_survives_refused_force() {
    let mut session = Session::open("/doc.md", "hello");
    let doc_id = session.app().active;

    assert!(session.key(ch('!')).is_none());
    assert_eq!(
        session.app().doc(doc_id).unwrap().buffer.content(),
        "!hello"
    );
    assert!(session.deliver_db().is_none());

    assert!(session.key(sup('s')).is_none());
    assert!(
        session.app().doc(doc_id).unwrap().save_in_flight(),
        "the real ⌘S must actually start a save"
    );

    let raise = rune_tui::guard::set_guard(
        session.app_mut(),
        rune_tui::guard::GuardPrompt {
            doc: doc_id,
            kind: GuardKind::DiskConflict,
        },
    );
    assert_eq!(raise, rune_tui::guard::GuardRaise::Raised);

    assert!(session.key(ch('s')).is_none());
    assert!(
        matches!(
            &session.app().guard,
            Some(prompt) if matches!(prompt.kind, GuardKind::DiskConflict)
        ),
        "a refused force-save must leave the disk-conflict prompt up"
    );
    let log = rune_tui::messages::log_text(session.app());
    assert!(
        log.contains("a save is already in progress"),
        "the refusal must be posted, not silent: {log:?}"
    );

    drain_materialize_round_trip(&mut session);
    assert!(
        !session.app().doc(doc_id).unwrap().save_in_flight(),
        "the original save must have finished against the quiet disk"
    );

    assert!(session.key(ch('s')).is_none());
    assert!(
        session.app().guard.is_none(),
        "a force-save that actually starts must clear the Guard"
    );
    drain_materialize_round_trip(&mut session);

    assert!(!session.app().doc(doc_id).unwrap().is_dirty());
    assert_eq!(
        session.app().vfs.read(Path::new("/doc.md")).unwrap(),
        b"!hello",
        "the retried force-save must still publish the buffer's bytes"
    );
}

/// The baseline-lifecycle fix: a commit whose own observation was lost to
/// a failing writer leaves `expect_obs` stale but
/// stashes the hash of what THIS session actually wrote
/// (`DocDb::pending_rebaseline_hash`) — simulating exactly that state here,
/// since reproducing the transient writer-queue failure that produces it for
/// real would make the test racy against the writer thread. The very next
/// save must recognize the disk as its own echo rather than manufacture a
/// conflict against it.
#[test]
fn a_baseline_left_unconfirmed_by_lost_bookkeeping_does_not_conflict_the_next_save() {
    let mut session = Session::open("/doc.md", "hello");
    let doc_id = session.app().active;

    assert!(session.key(ch('!')).is_none());
    assert_eq!(
        session.app().doc(doc_id).unwrap().buffer.content(),
        "!hello"
    );
    assert!(session.deliver_db().is_none());

    // The write this session is about to "retry" already physically landed
    // (a prior attempt's own commit, whose `MaterializeRecord` bookkeeping
    // never came back) — the disk holds exactly the buffer's own bytes.
    external_write(session.app().vfs.as_ref(), b"!hello");
    let db_id = session
        .app()
        .doc(doc_id)
        .and_then(|d| d.doc_db())
        .expect("the document is store-bound")
        .db_id;
    {
        let binding = session
            .app_mut()
            .file_binding_mut(db_id)
            .expect("the file has a shared baseline");
        binding.pending_rebaseline_hash = Some(rune_db::hash_bytes(b"!hello"));
        // `expect_obs` is deliberately left as the Load's own baseline — the
        // hash of the ORIGINAL "hello", never advanced — proving the stash,
        // not a coincidentally-matching `expect_obs`, is what lets this
        // save through.
    }

    save_and_ack(&mut session);

    assert!(
        session.app().guard.is_none(),
        "a baseline unconfirmed only because bookkeeping was lost must not \
         conflict against bytes this session itself just wrote"
    );
    assert!(!session.app().doc(doc_id).unwrap().is_dirty());
    assert_eq!(
        session
            .app()
            .file_binding(db_id)
            .unwrap()
            .pending_rebaseline_hash,
        None,
        "a real observation landing must clear the stand-in"
    );
}

/// The same fix must never let the next save silently adopt someone else's
/// bytes — a live disk that disagrees with the stashed hash still raises the
/// conflict Guard honestly.
#[test]
fn a_baseline_left_unconfirmed_by_lost_bookkeeping_still_conflicts_on_foreign_bytes() {
    let mut session = Session::open("/doc.md", "hello");
    let doc_id = session.app().active;

    assert!(session.key(ch('!')).is_none());
    assert!(session.deliver_db().is_none());

    // A DIFFERENT writer's bytes land on disk — not this session's own
    // stashed commit.
    external_write(session.app().vfs.as_ref(), b"someone else wrote this");
    {
        let db_id = session
            .app()
            .doc(doc_id)
            .and_then(|d| d.doc_db())
            .expect("the document is store-bound")
            .db_id;
        let binding = session
            .app_mut()
            .file_binding_mut(db_id)
            .expect("the file has a shared baseline");
        binding.pending_rebaseline_hash = Some(rune_db::hash_bytes(b"!hello"));
    }

    save_and_ack(&mut session);

    let Some(prompt) = &session.app().guard else {
        panic!("a stashed hash must never license adopting someone else's bytes");
    };
    assert!(matches!(prompt.kind, GuardKind::DiskConflict));
}
