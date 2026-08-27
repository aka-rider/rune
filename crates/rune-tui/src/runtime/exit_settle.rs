use std::sync::mpsc::Receiver;
use std::thread::{self, JoinHandle};

use crate::app::App;

use super::{Effects, Msg};

pub(crate) fn join_save_handles(
    app: &mut App,
    rx: &Receiver<Msg>,
    handles: &mut Vec<JoinHandle<()>>,
) {
    while !handles.is_empty() {
        settle_pending_materialize(app, rx);
        let (finished, still_running): (Vec<_>, Vec<_>) = std::mem::take(handles)
            .into_iter()
            .partition(JoinHandle::is_finished);
        *handles = still_running;
        for handle in finished {
            let _ = handle.join();
        }
        if !handles.is_empty() {
            thread::yield_now();
        }
    }
    settle_pending_materialize(app, rx);
}

pub(crate) fn settle_pending_materialize(app: &mut App, rx: &Receiver<Msg>) {
    let mut effects = Effects::default();
    while let Ok(msg) = rx.try_recv() {
        match msg {
            Msg::MaterializeVfsDone {
                id,
                ticket,
                db_id,
                seq,
                content,
                outcome,
            } => {
                crate::materialize_ack::handle_materialize_vfs_done(
                    app, id, ticket, db_id, seq, &content, outcome,
                );
            }
            Msg::SaveDone {
                id,
                ticket,
                version,
                result,
                detail,
            } => {
                crate::materialize_ack::handle_save_done(app, id, ticket, version, result, detail);
            }
            Msg::RenameDone { generation, result } => {
                crate::rename::handle_rename_done(app, generation, result, &mut effects);
            }
            Msg::TrashDone {
                generation,
                path,
                result,
            } => {
                crate::trash::handle_trash_done(app, generation, &path, result, &mut effects);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;
    use std::sync::mpsc;

    use rune_core::buffer::Buffer;
    use rune_vfs::{Mem, Vfs, VfsTestExt};

    use crate::app::{self, App};
    use crate::db::{Db, DbBridge};
    use crate::document::SavePhase;
    use crate::runtime::{CmdKind, Effects, Msg};
    use crate::save::{SaveMode, SaveOrigin, SaveStart};
    use crate::workspace;

    use super::{join_save_handles, settle_pending_materialize};

    fn drain_db_ops(app: &mut App, bridge: &DbBridge, effects: &mut Effects) {
        while !app.db_ops.is_empty() {
            let evt = bridge.wait_for_bootstrap_event(|_| true);
            app::update(app, Msg::Db(evt), effects);
        }
    }

    #[test]
    fn a_discard_quit_settles_an_undelivered_materialize_reply_before_exit() {
        let mem = Arc::new(Mem::new());
        mem.save_atomic(Path::new("/doc.md"), b"hello")
            .expect("seed doc.md");
        let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(&mem) as Arc<dyn Vfs + Send + Sync>;
        let bridge = DbBridge::bootstrap();
        let clock: rune_db::ClockFn = Arc::new(std::time::SystemTime::now);
        let store = rune_db::Store::open_in_memory(clock, Arc::clone(&vfs), bridge.on_event())
            .expect("open store");
        let db = Db::new(store, Arc::clone(&bridge), false);
        let mut app = App::new(Buffer::new(""), None, vfs, Some(db));
        let id = workspace::open_path(&mut app, Path::new("/doc.md")).expect("open doc.md");

        let mut effects = Effects::default();
        drain_db_ops(&mut app, &bridge, &mut effects);
        crate::commands::edit::insert_char(&mut app, id, 'X');
        assert_eq!(
            crate::save::trigger_save(
                &mut app,
                id,
                SaveMode::Normal,
                SaveOrigin::Interactive,
                &mut effects
            ),
            SaveStart::InFlight
        );
        drain_db_ops(&mut app, &bridge, &mut effects);
        let vfs_cmd = effects
            .cmds
            .drain(..)
            .find(|cmd| cmd.kind() == CmdKind::Save)
            .expect("the prepare ack spawns the publish Cmd");
        let reply = vfs_cmd.run().expect("the publish Cmd replies");
        assert!(matches!(reply, Msg::MaterializeVfsDone { .. }));

        app.should_quit = true;
        let (tx, rx) = mpsc::channel();
        tx.send(reply).expect("park the reply in the channel");

        assert!(
            matches!(
                app.doc(id).expect("doc open").save_phase(),
                SavePhase::Publishing
            ),
            "the reply is still unprocessed when the loop breaks"
        );
        assert!(app.db_ops.is_empty());

        settle_pending_materialize(&mut app, &rx);

        assert!(
            matches!(
                app.doc(id).expect("doc open").save_phase(),
                SavePhase::Recording { published: true }
            ),
            "the ack's bookkeeping must run before exit"
        );
        assert!(
            app.db_ops.values().any(|pending| pending.doc == id),
            "the MaterializeRecord op must be enqueued for the store's shutdown drain"
        );
        assert_eq!(
            mem.read(Path::new("/doc.md")).expect("read doc.md"),
            b"Xhello",
            "the publish itself already committed"
        );
    }

    #[test]
    fn a_save_thread_finishing_after_the_join_starts_still_lands_its_reply() {
        let mem = Arc::new(Mem::new());
        mem.save_atomic(Path::new("/doc.md"), b"hello")
            .expect("seed doc.md");
        let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(&mem) as Arc<dyn Vfs + Send + Sync>;
        let bridge = DbBridge::bootstrap();
        let clock: rune_db::ClockFn = Arc::new(std::time::SystemTime::now);
        let store = rune_db::Store::open_in_memory(clock, Arc::clone(&vfs), bridge.on_event())
            .expect("open store");
        let db = Db::new(store, Arc::clone(&bridge), false);
        let mut app = App::new(Buffer::new(""), None, vfs, Some(db));
        let id = workspace::open_path(&mut app, Path::new("/doc.md")).expect("open doc.md");

        let mut effects = Effects::default();
        drain_db_ops(&mut app, &bridge, &mut effects);
        crate::commands::edit::insert_char(&mut app, id, 'X');
        assert_eq!(
            crate::save::trigger_save(
                &mut app,
                id,
                SaveMode::Normal,
                SaveOrigin::Interactive,
                &mut effects
            ),
            SaveStart::InFlight
        );
        drain_db_ops(&mut app, &bridge, &mut effects);
        let vfs_cmd = effects
            .cmds
            .drain(..)
            .find(|cmd| cmd.kind() == CmdKind::Save)
            .expect("the prepare ack spawns the publish Cmd");
        let reply = vfs_cmd.run().expect("the publish Cmd replies");
        assert!(matches!(reply, Msg::MaterializeVfsDone { .. }));

        let (tx, rx) = mpsc::channel();
        let (gate_tx, gate_rx) = mpsc::channel::<()>();
        let save_thread = std::thread::spawn(move || {
            gate_rx.recv().expect("the test opens the gate");
            tx.send(reply).expect("the shutdown loop is still draining");
        });
        let mut handles = vec![save_thread];

        let (result_tx, result_rx) = mpsc::channel();
        let shutdown = std::thread::spawn(move || {
            join_save_handles(&mut app, &rx, &mut handles);
            result_tx
                .send(app)
                .expect("the test thread is still waiting for the result");
        });

        assert!(
            !shutdown.is_finished(),
            "the shutdown join must still be waiting: the save thread is \
             gated shut and has not sent its reply yet"
        );

        gate_tx.send(()).expect("release the gated save thread");
        shutdown.join().expect("the shutdown thread must not panic");

        let app = result_rx
            .recv()
            .expect("join_save_handles must return once the late save thread lands");
        assert!(
            matches!(
                app.doc(id).expect("doc open").save_phase(),
                SavePhase::Recording { published: true }
            ),
            "a save thread that finishes only after the join loop starts \
             must still have its reply applied before shutdown proceeds"
        );
    }

    #[test]
    fn a_late_save_done_failure_at_shutdown_is_surfaced_not_swallowed() {
        let mem = Arc::new(Mem::new());
        mem.save_atomic(Path::new("/doc.md"), b"hello")
            .expect("seed doc.md");
        let vfs: Arc<dyn Vfs + Send + Sync> = mem;
        let mut app = App::new(
            Buffer::new("hello"),
            Some(std::path::PathBuf::from("/doc.md")),
            vfs,
            None,
        );
        let id = app.active;
        let (version, content) = {
            let doc = app.doc(id).expect("doc open");
            (doc.buffer.version(), Arc::from(doc.buffer.content()))
        };
        let ticket = app
            .doc_mut(id)
            .expect("doc open")
            .begin_save(version, content);

        let (tx, rx) = mpsc::channel();
        tx.send(Msg::SaveDone {
            id,
            ticket,
            version,
            result: Err(crate::runtime::CmdError::Refused("disk full".to_string())),
            detail: crate::runtime::SaveOutcomeDetail {
                durable: true,
                stray_temp: None,
                race: None,
            },
        })
        .expect("park the reply in the channel");

        settle_pending_materialize(&mut app, &rx);

        assert_eq!(
            crate::messages::newest_text(&app),
            Some("save failed: disk full"),
            "a SaveDone(Err) drained at shutdown must still be reported, not discarded"
        );
    }
}
