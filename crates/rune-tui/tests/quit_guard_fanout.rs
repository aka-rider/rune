#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod dirty_common;
mod quit_guard_common;

use std::sync::Arc;

use rune_core::buffer::Buffer;
use rune_db::{ClockFn, Store};
use rune_tui::app::update;
use rune_tui::db::Db;
use rune_tui::guard::GuardKind;
use rune_tui::keymap::KeyCode;
use rune_tui::runtime::{Effects, Msg, SaveOutcomeDetail};
use rune_vfs::{Mem, Vfs, VfsTestExt};

use quit_guard_common::{ctrl_c, guard_kind, key, named_dirty_doc, press, resolved, test_app};

#[test]
fn two_dirty_docs_guard_save_quits_only_after_both_ack() {
    let mut app = test_app();
    let id_a = named_dirty_doc(&mut app, "/a.md");
    let b_path = resolved(&app, "/b.md");
    let id_b = app.open_document_bound(Buffer::new("second"), b_path);
    dirty_common::force_dirty(&mut app, id_b);
    let version_a = app.doc(id_a).unwrap().buffer.version();
    let version_b = app.doc(id_b).unwrap().buffer.version();

    press(&mut app, ctrl_c());
    press(&mut app, key(KeyCode::Char('s')));

    assert_eq!(app.quit.fan_out().map(|i| i.pending.len()), Some(2));
    assert!(app.doc(id_a).unwrap().save_in_flight());
    assert!(app.doc(id_b).unwrap().save_in_flight());

    let ticket_a = app.doc(id_a).unwrap().save_ticket().unwrap();
    let ticket_b = app.doc(id_b).unwrap().save_ticket().unwrap();
    let mut effects = Effects::default();
    update(
        &mut app,
        Msg::SaveDone {
            id: id_a,
            ticket: ticket_a,
            version: version_a,
            result: Ok(()),
            detail: SaveOutcomeDetail {
                durable: true,
                stray_temp: None,
                race: None,
            },
        },
        &mut effects,
    );
    assert!(
        !app.should_quit,
        "one ack out of two must not complete the quit"
    );

    update(
        &mut app,
        Msg::SaveDone {
            id: id_b,
            ticket: ticket_b,
            version: version_b,
            result: Ok(()),
            detail: SaveOutcomeDetail {
                durable: true,
                stray_temp: None,
                race: None,
            },
        },
        &mut effects,
    );
    assert!(
        app.should_quit,
        "the second, final ack must complete the quit"
    );
}

#[test]
fn closing_one_awaited_document_mid_flight_still_lets_the_quit_resolve() {
    let mut app = test_app();
    let id_a = named_dirty_doc(&mut app, "/a.md");
    let b_path = resolved(&app, "/b.md");
    let id_b = app.open_document_bound(Buffer::new("second"), b_path);
    dirty_common::force_dirty(&mut app, id_b);
    let version_a = app.doc(id_a).unwrap().buffer.version();

    press(&mut app, ctrl_c());
    press(&mut app, key(KeyCode::Char('s')));
    assert_eq!(app.quit.fan_out().map(|i| i.pending.len()), Some(2));

    let mut effects = Effects::default();
    let _ = rune_tui::workspace::close_now(&mut app, id_b, &mut effects);
    assert!(
        !app.should_quit,
        "closing one awaited document must not itself complete the quit \
         while the other is still outstanding"
    );
    assert_eq!(app.quit.fan_out().map(|i| i.pending.len()), Some(1));

    let ticket_a = app.doc(id_a).unwrap().save_ticket().unwrap();
    update(
        &mut app,
        Msg::SaveDone {
            id: id_a,
            ticket: ticket_a,
            version: version_a,
            result: Ok(()),
            detail: SaveOutcomeDetail {
                durable: true,
                stray_temp: None,
                race: None,
            },
        },
        &mut effects,
    );
    assert!(
        app.should_quit,
        "the remaining document's ack must still complete the quit"
    );
}

#[test]
fn store_failure_mid_quit_save_aborts_the_quit_and_the_next_ctrl_c_still_works() {
    let mut app = test_app();
    let id = named_dirty_doc(&mut app, "/a.md");

    press(&mut app, ctrl_c());
    let mut save_effects = Effects::default();
    update(
        &mut app,
        Msg::Key(key(KeyCode::Char('s'))),
        &mut save_effects,
    );
    assert!(app.quit.fan_out().is_some());
    let save_cmd = save_effects
        .cmds
        .into_iter()
        .find(|c| c.kind() == rune_tui::runtime::CmdKind::Save)
        .expect("the fan-out must have spawned a Save Cmd");

    let mut effects = Effects::default();
    update(
        &mut app,
        Msg::Db(rune_db::DbEvent::Fatal {
            error: "writer panicked".to_string(),
        }),
        &mut effects,
    );

    assert!(
        !app.should_quit,
        "a store failure must never let quit complete"
    );
    assert!(
        rune_tui::messages::newest_text(&app).is_some(),
        "the failure must be surfaced"
    );
    assert!(
        app.doc(id).unwrap().save_in_flight(),
        "a Direct save's own vfs Cmd is unrelated to the store — a store \
         failure must leave it running, not abandon it out from under the \
         write already headed to disk"
    );
    let save_done = save_cmd.run().expect("the Save Cmd must reply");
    let mut effects3 = Effects::default();
    update(&mut app, save_done, &mut effects3);
    assert!(
        !app.doc(id).unwrap().save_in_flight(),
        "the Direct save's own ack must resolve it once it actually lands"
    );
    assert!(
        app.quit.fan_out().is_none(),
        "the stranded intent must be cleared"
    );

    assert!(!app.doc(id).unwrap().is_dirty());
    press(&mut app, ctrl_c());
    press(&mut app, ctrl_c());
    assert!(
        app.should_quit,
        "the next ^C presses must still be responsive rather than silently \
         doing nothing"
    );
}

#[test]
fn two_dirty_docs_degraded_store_arms_exactly_one_confirm_gate() {
    let mem = Arc::new(Mem::new());
    {
        let vfs: &dyn Vfs = mem.as_ref();
        vfs.save_atomic(std::path::Path::new("/a.md"), b"hello")
            .expect("seed a.md");
        vfs.save_atomic(std::path::Path::new("/b.md"), b"second")
            .expect("seed b.md");
    }
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(&mem) as Arc<dyn Vfs + Send + Sync>;
    let clock: ClockFn = Arc::new(std::time::SystemTime::now);
    let bridge = rune_tui::db::DbBridge::bootstrap();
    let store =
        Store::open_in_memory(clock, Arc::clone(&vfs), bridge.on_event()).expect("open store");
    let db = Db::new(store, Arc::clone(&bridge), false);

    let mut session = rune_fuzz::Session::open_with_db("/a.md", Arc::clone(&mem), db);
    let id_a = session.app().active;
    let id_b = rune_tui::workspace::open_path(session.app_mut(), std::path::Path::new("/b.md"))
        .expect("open b.md");
    assert!(session.deliver_db_all().is_none());
    assert!(session.app().doc(id_a).unwrap().is_store_bound());
    assert!(session.app().doc(id_b).unwrap().is_store_bound());

    let app = session.app_mut();
    let mut effects = Effects::default();
    update(
        app,
        Msg::Db(rune_db::DbEvent::Fatal {
            error: "writer panicked".to_string(),
        }),
        &mut effects,
    );
    assert!(
        app.db.as_ref().is_some_and(|db| db.degraded),
        "test setup: the store must be degraded"
    );

    dirty_common::force_dirty(app, id_a);
    dirty_common::force_dirty(app, id_b);

    press(app, ctrl_c());
    assert_eq!(guard_kind(app), Some(&GuardKind::DirtyQuit));
    press(app, key(KeyCode::Char('s')));

    assert!(
        app.pending_save_confirm.is_some(),
        "exactly one confirm gate must be armed"
    );
    let (armed_id, _) = app.pending_save_confirm.expect("checked above");
    assert!(
        armed_id == id_a || armed_id == id_b,
        "the armed gate must name one of the two dirty documents"
    );
    assert!(
        !app.doc(id_a).unwrap().save_in_flight() && !app.doc(id_b).unwrap().save_in_flight(),
        "the degraded arm must never enqueue a save on its first press"
    );
    let expected_name = if armed_id == id_a { "a.md" } else { "b.md" };
    assert!(
        rune_tui::messages::newest_text(app)
            .is_some_and(|m| m.contains("recovery disabled") && m.contains(expected_name)),
        "the status must name the SAME document the confirm gate is armed for, got {:?}",
        rune_tui::messages::newest_text(app)
    );
    assert!(
        app.quit.fan_out().is_none(),
        "no save actually started, so no quit intent may be left waiting"
    );
    assert!(!app.should_quit);
}
