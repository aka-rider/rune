//! `Document::save`'s state machine makes two invariants structurally
//! impossible to violate. (1) While a publish `Cmd` is outstanding for a
//! document, no second publish can be spawned for it. (2) No `App`-level
//! map may reference a closed document's save attempt — trivially true now
//! since the attempt lives on the `Document` itself and is removed with it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod rename_common;

use std::sync::Arc;

use rune_db::DbEvent;
use rune_tui::document::SavePhase;
use rune_tui::runtime::{Cmd, CmdKind, Effects, Msg};
use rune_vfs::Vfs;

use rename_common::{app_with_store, seeded_vfs, send, sup, type_text, wait_for_materialize_prep};

/// Drives `app`'s active document from `Idle` through a real ⌘S all the way
/// to `Publishing` — the prep ack has landed, the caller-side vfs `Cmd` is
/// outstanding — and returns that `Cmd` without running it.
fn drive_to_publishing(app: &mut rune_tui::app::App, bridge: &Arc<rune_tui::db::DbBridge>) -> Cmd {
    send(app, sup('s'));
    let prep_evt = wait_for_materialize_prep(bridge);
    let mut effects = send(app, Msg::Db(prep_evt));
    effects
        .cmds
        .drain(..)
        .find(|c| c.kind() == CmdKind::Save)
        .expect("the prepare ack must spawn the caller-side vfs Cmd")
}

/// Invariant (1): a second save attempt must be refused outright while the
/// first one's own publish `Cmd` is still outstanding — including right
/// after a whole-store failure, which must leave `Publishing` untouched
/// rather than releasing the document for a second attempt to clobber.
#[test]
fn second_save_refused_while_publishing() {
    let mem = seeded_vfs();
    let (mut app, bridge) = app_with_store(&mem);
    let id = app.active;
    type_text(&mut app, "!");
    let first_content = app.doc(id).unwrap().buffer.content().to_string();

    let cmd = drive_to_publishing(&mut app, &bridge);
    assert_eq!(app.doc(id).unwrap().save_phase(), SavePhase::Publishing);

    send(
        &mut app,
        Msg::Db(DbEvent::Fatal {
            error: "writer died".to_string(),
        }),
    );
    assert_eq!(
        app.doc(id).unwrap().save_phase(),
        SavePhase::Publishing,
        "a store failure must leave a Publishing save completely untouched"
    );

    // A second attempt at the SAME document (⌘S again) must be refused —
    // no second `MaterializePrepare` may enqueue while the first one's own
    // publish is still outstanding.
    let ops_before = app.db_ops.len();
    send(&mut app, sup('s'));
    assert_eq!(
        app.db_ops.len(),
        ops_before,
        "a second save attempt must never enqueue a second op while Publishing"
    );
    assert_eq!(
        app.doc(id).unwrap().save_phase(),
        SavePhase::Publishing,
        "the refused second attempt must never disturb the first one's own state"
    );
    assert!(
        rune_tui::messages::newest_text(&app).is_some_and(|s| s.contains("already in progress")),
        "the refusal must be surfaced"
    );

    // The FIRST attempt's own vfs Cmd finishes — it must resolve with the
    // FIRST capture, not anything a (refused) second attempt might have
    // captured.
    let vfs_done = cmd.run().expect("the vfs Cmd must reply");
    send(&mut app, vfs_done);
    let record_evt = rename_common::wait_for_materialize_record(&bridge);
    send(&mut app, Msg::Db(record_evt));

    assert!(!app.doc(id).unwrap().is_dirty());
    assert_eq!(
        mem.read(std::path::Path::new("/root/a.md")).unwrap(),
        first_content.as_bytes(),
        "the published bytes must be exactly the first attempt's own capture"
    );
}

/// A `Msg::MaterializeVfsDone` carrying a ticket this document no longer
/// recognizes (a stale reply for an attempt already resolved) must never
/// promote `saved_content` or enqueue a tracked `MaterializeRecord` — a
/// typed, silent drop.
#[test]
fn stale_vfs_done_never_promotes() {
    let mem = seeded_vfs();
    let (mut app, bridge) = app_with_store(&mem);
    let id = app.active;
    type_text(&mut app, "!");

    let cmd = drive_to_publishing(&mut app, &bridge);
    let stale_ticket = app.doc(id).unwrap().save_ticket().unwrap();

    // Resolve this attempt for real, so the ticket above is now stale.
    let vfs_done = cmd.run().expect("the vfs Cmd must reply");
    send(&mut app, vfs_done);
    let record_evt = rename_common::wait_for_materialize_record(&bridge);
    send(&mut app, Msg::Db(record_evt));
    assert_eq!(app.doc(id).unwrap().save_phase(), SavePhase::Idle);
    let saved_content_before = app.doc(id).unwrap().buffer.content().to_string();
    let ops_before = app.db_ops.len();
    let db_id = app.doc(id).unwrap().doc_db().unwrap().db_id;

    // A late reply for the now-stale ticket, claiming a DIFFERENT commit —
    // must be dropped without touching anything.
    send(
        &mut app,
        Msg::MaterializeVfsDone {
            id,
            ticket: stale_ticket,
            db_id,
            seq: 0,
            content: Arc::from("an attacker's stale bytes"),
            outcome: rune_tui::materialize_ack::MaterializeVfsOutcome::Missing,
        },
    );

    assert_eq!(
        app.doc(id).unwrap().buffer.content(),
        saved_content_before,
        "a stale ticket must never promote anything"
    );
    assert_eq!(
        app.db_ops.len(),
        ops_before,
        "a stale ticket must never enqueue a tracked op"
    );
    assert_eq!(app.doc(id).unwrap().save_phase(), SavePhase::Idle);
}

/// Invariant (2): closing a document while its save is `Preparing`,
/// `Publishing`, or `Recording` leaves no `App`-level map referencing it —
/// every later stale ack/message for that attempt is a silent, panic-free
/// no-op.
#[test]
fn close_mid_save_leaves_no_state() {
    for phase in ["preparing", "publishing", "recording"] {
        let mem = seeded_vfs();
        let (mut app, bridge) = app_with_store(&mem);
        let id = app.active;
        type_text(&mut app, "!");

        send(&mut app, sup('s'));
        let prep_op = *app
            .db_ops
            .iter()
            .find(|(_, pending)| pending.doc == id)
            .expect("a MaterializePrepare op must be tracked")
            .0;

        let mut cmd = if phase == "preparing" {
            None
        } else {
            let prep_evt = wait_for_materialize_prep(&bridge);
            let mut effects = send(&mut app, Msg::Db(prep_evt));
            Some(
                effects
                    .cmds
                    .drain(..)
                    .find(|c| c.kind() == CmdKind::Save)
                    .expect("the prepare ack must spawn the caller-side vfs Cmd"),
            )
        };

        let record_op = if phase == "recording" {
            let vfs_done = cmd
                .take()
                .expect("publishing cmd present")
                .run()
                .expect("vfs Cmd replies");
            send(&mut app, vfs_done);
            Some(
                *app.db_ops
                    .iter()
                    .find(|(_, pending)| pending.doc == id)
                    .expect("a MaterializeRecord op must be tracked")
                    .0,
            )
        } else {
            None
        };

        let mut effects = Effects::default();
        let _ = rune_tui::workspace::close_now(&mut app, id, &mut effects);
        assert!(!app.documents.contains_key(&id), "case {phase}");
        assert!(
            !app.db_ops.values().any(|pending| pending.doc == id),
            "case {phase}: db_ops must not reference a closed document"
        );

        // Every stale reply for the closed document's attempt must be a
        // silent no-op — never a panic, never a reaction.
        send(
            &mut app,
            Msg::Db(DbEvent::Ok {
                id: prep_op,
                result: rune_db::OpOutcome::MaterializePrep(Box::new(rune_db::MaterializePrep {
                    expect_hash: String::new(),
                    bound_path: None,
                })),
            }),
        );
        if let Some(cmd) = cmd {
            let vfs_done = cmd.run().expect("vfs Cmd replies");
            send(&mut app, vfs_done);
        }
        if let Some(record_op) = record_op {
            send(
                &mut app,
                Msg::Db(DbEvent::Ok {
                    id: record_op,
                    result: rune_db::OpOutcome::Materialize(Box::new(rune_db::MatResult {
                        committed: true,
                        ..Default::default()
                    })),
                }),
            );
        }
        assert!(
            !app.documents.contains_key(&id),
            "case {phase}: still closed"
        );
    }
}

/// A store failure landing while `Publishing` must not abandon the
/// document — the eventual `MaterializeVfsDone` + `MaterializeRecord` path
/// still settles it, with the correct (first-captured) bytes, exactly as
/// if the store had never failed.
#[test]
fn store_failure_mid_publish_does_not_abandon() {
    let mem = seeded_vfs();
    let (mut app, bridge) = app_with_store(&mem);
    let id = app.active;
    type_text(&mut app, "!");
    let content = app.doc(id).unwrap().buffer.content().to_string();

    let cmd = drive_to_publishing(&mut app, &bridge);

    send(
        &mut app,
        Msg::Db(DbEvent::Fatal {
            error: "writer died mid-publish".to_string(),
        }),
    );
    assert!(
        app.doc(id).unwrap().save_in_flight(),
        "Publishing must survive a store failure"
    );
    assert!(app.db.as_ref().unwrap().degraded);

    // The vfs write itself never depended on the (now-degraded) store — it
    // still completes, advances Publishing to Recording, and the eventual
    // MaterializeRecord ack settles it with the correct, first-captured
    // bytes, exactly as if the store had never failed.
    let vfs_done = cmd.run().expect("the vfs Cmd must reply");
    send(&mut app, vfs_done);
    assert_eq!(
        app.doc(id).unwrap().save_phase(),
        SavePhase::Recording { published: true }
    );
    let record_evt = rename_common::wait_for_materialize_record(&bridge);
    send(&mut app, Msg::Db(record_evt));

    assert!(!app.doc(id).unwrap().save_in_flight());
    assert!(!app.doc(id).unwrap().is_dirty());
    assert_eq!(
        mem.read(std::path::Path::new("/root/a.md")).unwrap(),
        content.as_bytes(),
        "the write must have reached disk despite the dead writer"
    );
    assert!(
        !rune_tui::messages::log_text(&app).contains("save failed"),
        "a physically-successful write must never be reported as a failed save"
    );
}
