use std::sync::mpsc::Receiver;

use crate::app::App;

use super::Msg;

/// Must run after every `CmdKind::Save` handle is joined — the join is what
/// guarantees an in-flight publish's `MaterializeVfsDone` reply has already
/// been sent, so this bounded drain settles its bookkeeping before the
/// process exits instead of orphaning a write that already reached disk.
pub(crate) fn settle_pending_materialize(app: &mut App, rx: &Receiver<Msg>) {
    while let Ok(msg) = rx.try_recv() {
        if let Msg::MaterializeVfsDone {
            id,
            ticket,
            db_id,
            seq,
            content,
            outcome,
        } = msg
        {
            crate::materialize_ack::handle_materialize_vfs_done(
                app, id, ticket, db_id, seq, &content, outcome,
            );
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
    use rune_vfs::{Mem, Vfs};

    use crate::app::{self, App};
    use crate::db::{Db, DbBridge};
    use crate::document::SavePhase;
    use crate::runtime::{CmdKind, Effects, Msg};
    use crate::save::{SaveMode, SaveOrigin, SaveStart};
    use crate::workspace;

    use super::settle_pending_materialize;

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
}
