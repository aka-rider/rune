//! Integration tests for the rune-tui <-> rune-db wiring's
//! degraded-store banner and its `super+s` confirm gate,
//! driven through `rune_fuzz::Session`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::Path;

use rune_fuzz::Session;
use rune_tui::app::update;
use rune_tui::footer;
use rune_tui::generation::Generation;
use rune_tui::keymap::{KeyCode, KeyInput, Mods};
use rune_tui::runtime::{Effects, Msg};

const END: KeyInput = KeyInput {
    code: KeyCode::End,
    mods: Mods::NONE,
};

const SAVE: KeyInput = KeyInput {
    code: KeyCode::Char('s'),
    mods: Mods {
        shift: false,
        alt: false,
        ctrl: false,
        sup: true,
    },
};

fn ch(c: char) -> KeyInput {
    KeyInput {
        code: KeyCode::Char(c),
        mods: Mods::NONE,
    }
}

/// Type -> kill the store writer via the test hook
/// (`Store::kill_writer_for_test`) -> the persistent degraded banner
/// appears in `footer::footer_text`'s output, and the buffer's content is
/// NEVER rolled back (an enqueue-time failure only
/// degrades the store — it never touches the in-memory buffer/journal).
#[test]
fn killed_writer_surfaces_a_degraded_banner_without_rolling_back_the_buffer() {
    let mut session = Session::open("/doc.md", "hi");

    // Typing while the store is healthy journals normally — no banner.
    assert!(session.key(END).is_none());
    assert!(session.type_("!").is_none());
    assert_eq!(session.snapshot().content, "hi!");
    assert!(session.app().db_banner.is_none());

    session
        .app()
        .db
        .as_ref()
        .expect("app has a store")
        .store
        .kill_writer_for_test()
        .expect("enqueue the kill op");

    // Keep typing until the (now-dying) writer's enqueue failure surfaces
    // the banner. Bounded spin, not a wall-clock sleep (repo convention):
    // the kill op must first be DEQUEUED by the writer thread before it
    // takes effect, so exactly how many further enqueues still succeed is
    // a genuine race, not something a fixed count can predict up front.
    let mut typed = String::from("hi!");
    let mut saw_banner = false;
    for i in 0..2000 {
        let c = if i % 2 == 0 { 'a' } else { 'b' };
        assert!(session.key(ch(c)).is_none());
        typed.push(c);
        if session.app().db_banner.is_some() {
            saw_banner = true;
            break;
        }
    }

    assert!(
        saw_banner,
        "the degraded banner must appear once the writer is confirmed gone"
    );
    let app = session.app();
    assert!(
        app.db_banner
            .as_deref()
            .is_some_and(|b| b.contains("recovery disabled")),
        "banner text must read 'recovery disabled: <err>' (got {:?})",
        app.db_banner
    );
    assert!(
        footer::footer_text(app).contains("recovery disabled"),
        "the banner must be part of the rendered footer line"
    );
    assert!(
        app.db.as_ref().is_some_and(|d| d.degraded),
        "the store must be marked degraded"
    );
    // No rollback, ever: the buffer must reflect EVERY
    // keystroke typed so far, regardless of exactly when the writer died
    // relative to these presses.
    assert_eq!(
        app.active_doc().buffer.content(),
        typed,
        "a store failure must never roll back the in-memory buffer"
    );
}

/// A `MaterializePrepare` enqueue failure
/// (the store writer confirmed gone) must degrade the store and raise the
/// sticky banner through the SAME `on_store_failure` chokepoint
/// `append_edit`/`move_undo_pos` use — never a one-shot `SaveError` status
/// that leaves `db.degraded` untouched. A dead writer must not ALSO make
/// the save itself impossible — `materialize_now` falls back to the
/// same uncoordinated direct-`vfs` `Cmd` a document with no store binding
/// uses, and once that `Cmd` runs, the user's bytes are actually on disk —
/// "press ⌘S again to save anyway" must actually save. Deterministically
/// waits for the writer to be CONFIRMED gone via a BLOCKING probe send
/// that is woken with `Err(WriterGone)` only when the writer drops its
/// queue receiver — a full queue merely parks the wait and never counts as
/// confirmation — before pressing save exactly once, rather than racing
/// `super+s`'s own in-flight latch against the kill op's async dequeue.
#[test]
fn a_dead_writer_thread_still_lets_the_save_reach_disk() {
    let mut session = Session::open("/doc.md", "hi");

    // Dirty the buffer (a healthy edit — the writer is still alive here) so
    // the save below actually has something to save.
    assert!(session.key(END).is_none());
    assert!(session.type_("!").is_none());
    assert!(session.app().db_banner.is_none());

    let doc_id = rune_db::DocId(
        session
            .app()
            .active_doc()
            .doc_db()
            .expect("the seed document is store-bound after setup")
            .db_id,
    );
    let db = session.app().db.as_ref().expect("app has a store");
    db.store
        .kill_writer_for_test()
        .expect("enqueue the kill op");

    // Condition-driven wait, not a spin and not a wall-clock sleep: the
    // kill op only takes effect once the writer thread DEQUEUES it, and
    // the one signal that has happened is the writer dropping its side of
    // the queue — which is exactly what wakes a blocking send. Each `Ok`
    // means the probe was accepted (a live writer drained a slot, or a
    // freed slot absorbed it); the writer is FIFO-bound to reach the kill
    // op after finitely many of those, so this loop terminates by writer
    // progress alone. Exhausting the cap means the writer survived without
    // ever reaching the kill op — a real failure to report loudly, not a
    // hang.
    let max_attempts = 4 * rune_db::QUEUE_DEPTH;
    let mut writer_confirmed_gone = false;
    for attempt in 0..=max_attempts {
        match db.store.probe_blocking_for_test(doc_id) {
            Ok(_) => assert!(
                attempt < max_attempts,
                "writer never confirmed dead after {max_attempts} blocking probes — \
                 it should have dequeued the kill op long before this"
            ),
            Err(rune_db::Error::WriterGone) => {
                writer_confirmed_gone = true;
                break;
            }
            Err(e) => panic!("unexpected error while awaiting writer death: {e}"),
        }
    }
    assert!(writer_confirmed_gone, "writer death was never confirmed");

    // Exactly ONE super+s now: `trigger_save`'s `materialize_now` enqueues
    // against a writer already confirmed gone, so this single call's
    // enqueue failure — and therefore `on_store_failure` — is deterministic,
    // never racing the in-flight latch `save_in_flight` would otherwise
    // impose on a retry loop.
    assert!(session.key(SAVE).is_none());

    let app = session.app();
    assert!(
        app.db_banner
            .as_deref()
            .is_some_and(|b| b.contains("recovery disabled")),
        "banner text must read 'recovery disabled: <err>' (got {:?})",
        app.db_banner
    );
    assert!(
        app.db.as_ref().is_some_and(|d| d.degraded),
        "the store must be marked degraded via on_store_failure, not left untouched"
    );
    assert!(
        app.active_doc().save_in_flight(),
        "a dead writer must not also make the save itself impossible — the \
         direct-vfs fallback Cmd is in flight, not silently skipped"
    );

    // `deliver` runs the deferred fallback Cmd and feeds its reply back —
    // the dead-writer fallback must have spawned one, and its ack must
    // land the user's bytes on disk.
    assert!(session.deliver().is_none());
    assert!(
        !session.app().active_doc().save_in_flight(),
        "the fallback save's own ack must clear save_in_flight"
    );
    assert_eq!(
        session
            .app()
            .vfs
            .read(Path::new("/doc.md"))
            .expect("file still readable"),
        b"hi!",
        "the user's edit must have reached disk despite the dead writer thread"
    );
}

/// The confirm-gate state machine: `super+s` on a degraded
/// store only ARMS the gate (no `materialize` enqueued, `save_in_flight`
/// stays false) the first time; a SECOND `super+s` consumes the gate and
/// actually enqueues the save. The store starts healthy and is flipped
/// degraded by hand — simulating a LATER store failure,
/// independent of the open ladder's own state, which is exactly what this
/// gate must react to either way.
#[test]
fn super_s_on_a_degraded_store_arms_a_confirm_gate_then_saves_on_second_press() {
    let mut session = Session::open("/doc.md", "hi");
    session
        .app_mut()
        .db
        .as_mut()
        .expect("app has a store")
        .degraded = true;

    // Dirty the buffer — nothing to save otherwise.
    assert!(session.key(END).is_none());
    assert!(session.type_("!").is_none());

    assert!(session.key(SAVE).is_none());
    assert!(
        session.app().pending_save_confirm.is_some(),
        "the first super+s on a degraded store must only arm the confirm gate"
    );
    assert!(
        !session.app().active_doc().save_in_flight(),
        "no materialize must be enqueued before the gate is confirmed"
    );
    assert!(
        rune_tui::messages::newest_text(session.app())
            .is_some_and(|s| s.contains("recovery disabled"))
    );

    assert!(session.key(SAVE).is_none());
    assert!(
        session.app().pending_save_confirm.is_none(),
        "the second super+s must consume the confirm gate"
    );
    assert!(
        session.app().active_doc().save_in_flight(),
        "the second super+s must actually enqueue the materialize"
    );
}

/// `super+s`'s confirm-gate arm (above) now arms its 2s timeout directly on
/// `App::timers` rather than spawning its own `Cmd` — this covers the
/// production `Msg` reaction on both sides of that timer's generation
/// check: a stale `Msg::SaveConfirmTimeout` (a generation that isn't the
/// one currently armed) must leave the gate standing, and the CURRENT
/// generation must clear it, exactly like the real timer thread's eventual
/// fire would.
#[test]
fn save_confirm_timeout_discards_a_stale_generation_and_fires_the_current_one() {
    let mut session = Session::open("/doc.md", "hi");
    session
        .app_mut()
        .db
        .as_mut()
        .expect("app has a store")
        .degraded = true;

    assert!(session.key(END).is_none());
    assert!(session.type_("!").is_none());
    assert!(session.key(SAVE).is_none());

    let (id, generation) = session
        .app()
        .pending_save_confirm
        .expect("the first super+s armed the confirm gate");

    let mut effects = Effects::default();
    update(
        session.app_mut(),
        Msg::SaveConfirmTimeout {
            generation: Generation::from_raw(generation.raw() + 1),
        },
        &mut effects,
    );
    assert_eq!(
        session.app().pending_save_confirm,
        Some((id, generation)),
        "a stale generation must never clear a still-current confirm gate"
    );

    let mut effects = Effects::default();
    update(
        session.app_mut(),
        Msg::SaveConfirmTimeout { generation },
        &mut effects,
    );
    assert!(
        session.app().pending_save_confirm.is_none(),
        "the current generation must clear the confirm gate"
    );
}
