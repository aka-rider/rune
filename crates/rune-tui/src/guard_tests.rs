#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;
use crate::app::App;
use crate::db::{Db, DocDb};
use crate::document::Replica;
use rune_core::buffer::Buffer;
use rune_db::{ClockFn, Store};
use rune_vfs::Mem;
use std::sync::Arc;

fn app() -> App {
    App::new(Buffer::new("hi"), None, Arc::new(Mem::new()), None)
}

fn store_bound_app(degraded: bool) -> App {
    let vfs: Arc<dyn rune_vfs::Vfs + Send + Sync> = Arc::new(Mem::new());
    let clock: ClockFn = Arc::new(std::time::SystemTime::now);
    let store = Store::open_in_memory(clock, Arc::clone(&vfs), Box::new(|_evt| {}))
        .expect("open in-memory store");
    let bridge = crate::db::DbBridge::bootstrap();
    let db = Db::new(store, bridge, degraded);
    let doc_db = DocDb::new(1, crate::db::PublishMode::CreateOnly, rune_db::Seq(0));
    let mut app = App::new(
        Buffer::new("hi"),
        Some(PathBuf::from("/doc.md")),
        vfs,
        Some(db),
    );
    let id = app.active;
    app.doc_mut(id).unwrap().replica = Replica::Bound(doc_db);
    app.install_or_join_file_binding(1, None);
    app
}

fn prompt(doc: DocumentId, kind: GuardKind) -> GuardPrompt {
    GuardPrompt { doc, kind }
}

#[test]
fn set_guard_raises_onto_an_empty_slot() {
    let mut app = app();
    let doc = app.active;
    assert_eq!(
        set_guard(
            &mut app,
            prompt(doc, GuardKind::DirtyClose),
            &mut crate::runtime::Effects::default()
        ),
        GuardRaise::Raised
    );
    assert!(matches!(
        app.guard,
        Some(GuardPrompt {
            kind: GuardKind::DirtyClose,
            ..
        })
    ));
}

#[test]
fn set_guard_refuses_to_replace_an_existing_prompt() {
    let mut app = app();
    let doc = app.active;
    assert_eq!(
        set_guard(
            &mut app,
            prompt(doc, GuardKind::DirtyClose),
            &mut crate::runtime::Effects::default()
        ),
        GuardRaise::Raised
    );
    assert_eq!(
        set_guard(
            &mut app,
            prompt(
                doc,
                GuardKind::RenameCollision {
                    target: "b.md".to_string(),
                }
            ),
            &mut crate::runtime::Effects::default()
        ),
        GuardRaise::Displaced
    );
    assert!(matches!(
        app.guard,
        Some(GuardPrompt {
            kind: GuardKind::DirtyClose,
            ..
        })
    ));
}

#[test]
fn clear_guard_empties_the_slot() {
    let mut app = app();
    let doc = app.active;
    assert_eq!(
        set_guard(
            &mut app,
            prompt(doc, GuardKind::DirtyClose),
            &mut crate::runtime::Effects::default()
        ),
        GuardRaise::Raised
    );
    clear_guard(&mut app);
    assert!(app.guard.is_none());
}

#[test]
fn retract_disk_conflict_on_convergence_is_a_noop_while_still_divergent() {
    let mut app = app();
    let doc = app.active;
    assert_eq!(
        set_guard(
            &mut app,
            prompt(doc, GuardKind::DiskConflict),
            &mut crate::runtime::Effects::default()
        ),
        GuardRaise::Raised
    );

    retract_disk_conflict_on_convergence(&mut app, doc, rune_db::SyncKind::Diverged);

    assert!(matches!(
        app.guard,
        Some(GuardPrompt {
            kind: GuardKind::DiskConflict,
            ..
        })
    ));
}

#[test]
fn retract_disk_conflict_on_convergence_clears_the_prompt() {
    let mut app = app();
    let doc = app.active;
    assert_eq!(
        set_guard(
            &mut app,
            prompt(doc, GuardKind::DiskConflict),
            &mut crate::runtime::Effects::default()
        ),
        GuardRaise::Raised
    );

    retract_disk_conflict_on_convergence(&mut app, doc, rune_db::SyncKind::Clean);

    assert!(app.guard.is_none());
    assert_eq!(
        messages::newest_text(&app),
        Some("disk settled — save again when ready")
    );
}

#[test]
fn retract_disk_conflict_on_convergence_touches_only_its_own_kind_and_doc() {
    let mut app = app();
    let doc = app.active;
    assert_eq!(
        set_guard(
            &mut app,
            prompt(doc, GuardKind::DirtyClose),
            &mut crate::runtime::Effects::default()
        ),
        GuardRaise::Raised
    );

    retract_disk_conflict_on_convergence(&mut app, doc, rune_db::SyncKind::Clean);

    assert!(matches!(
        app.guard,
        Some(GuardPrompt {
            kind: GuardKind::DirtyClose,
            ..
        })
    ));
}

#[test]
fn clear_guard_on_a_rename_collision_returns_the_rename_machine_to_idle() {
    let mut app = app();
    let doc = app.active;
    app.rename = crate::rename::RenameState::Collision {
        doc,
        from: std::path::PathBuf::new(),
        to: std::path::PathBuf::from("/b.md"),
        seen: rune_vfs::Stat {
            size: 0,
            mtime: std::time::SystemTime::UNIX_EPOCH,
            identity: rune_vfs::Identity::default(),
            nlink: None,
            kind: rune_vfs::FileKind::File,
        },
    };
    assert_eq!(
        set_guard(
            &mut app,
            prompt(
                doc,
                GuardKind::RenameCollision {
                    target: "b.md".to_string(),
                }
            ),
            &mut crate::runtime::Effects::default()
        ),
        GuardRaise::Raised
    );
    clear_guard(&mut app);
    assert!(app.guard.is_none());
    assert_eq!(app.rename, crate::rename::RenameState::Idle);
}

#[test]
fn set_guard_or_warn_posts_refused_text_and_preserves_the_existing_prompt() {
    let mut app = app();
    let doc = app.active;
    assert_eq!(
        set_guard(
            &mut app,
            prompt(doc, GuardKind::DiskConflict),
            &mut crate::runtime::Effects::default()
        ),
        GuardRaise::Raised
    );

    let raise = set_guard_or_warn(
        &mut app,
        prompt(doc, GuardKind::DirtyClose),
        "some confirmation dropped \u{2014} a prompt is already showing",
        &mut crate::runtime::Effects::default(),
    );

    assert_eq!(raise, GuardRaise::Displaced);
    assert!(matches!(
        app.guard,
        Some(GuardPrompt {
            kind: GuardKind::DiskConflict,
            ..
        })
    ));
    assert_eq!(
        messages::newest_text(&app),
        Some("some confirmation dropped \u{2014} a prompt is already showing")
    );
}

#[test]
fn set_guard_or_warn_raises_silently_onto_an_empty_slot() {
    let mut app = app();
    let doc = app.active;

    let raise = set_guard_or_warn(
        &mut app,
        prompt(doc, GuardKind::DirtyClose),
        "unused",
        &mut crate::runtime::Effects::default(),
    );

    assert_eq!(raise, GuardRaise::Raised);
    assert_eq!(messages::newest_text(&app), None);
}

#[test]
fn force_save_single_press_when_degraded() {
    let mut app = store_bound_app(true);
    let doc = app.active;
    crate::commands::edit::insert_char(&mut app, doc, '!');
    let mut effects = Effects::default();

    assert_eq!(
        save::trigger_save(
            &mut app,
            doc,
            SaveMode::Force,
            SaveOrigin::Guard,
            &mut effects
        ),
        SaveStart::InFlight
    );
    assert!(
        app.doc(doc).unwrap().save_in_flight(),
        "a Force save on a degraded store must reach materialize directly"
    );
    assert!(
        app.pending_save_confirm.is_none(),
        "Force must never arm the degraded confirm gate"
    );
}

#[test]
fn normal_save_still_arms_the_degraded_confirm_gate() {
    let mut app = store_bound_app(true);
    let doc = app.active;
    crate::commands::edit::insert_char(&mut app, doc, '!');
    let mut effects = Effects::default();

    assert_eq!(
        save::trigger_save(
            &mut app,
            doc,
            SaveMode::Normal,
            SaveOrigin::Interactive,
            &mut effects
        ),
        SaveStart::Refused
    );
    assert!(!app.doc(doc).unwrap().save_in_flight());
    assert!(app.pending_save_confirm.is_some_and(|(cid, _)| cid == doc));
}

#[test]
fn force_save_bypasses_not_dirty() {
    let mut app = store_bound_app(false);
    let doc = app.active;
    assert!(!app.doc(doc).unwrap().is_dirty());
    let mut effects = Effects::default();

    assert_eq!(
        save::trigger_save(
            &mut app,
            doc,
            SaveMode::Normal,
            SaveOrigin::Interactive,
            &mut effects
        ),
        SaveStart::NotDirty
    );

    let mut effects = Effects::default();
    assert_eq!(
        save::trigger_save(
            &mut app,
            doc,
            SaveMode::Force,
            SaveOrigin::Guard,
            &mut effects
        ),
        SaveStart::InFlight
    );
    assert!(app.doc(doc).unwrap().save_in_flight());
}

#[test]
fn an_unfocused_search_bar_survives_a_guard_raise() {
    let mut app = app();
    let doc = app.active;
    crate::search::open(&mut app, &mut crate::runtime::Effects::default());
    app.search_mut().expect("the bar is open").focused = false;

    assert_eq!(
        set_guard(
            &mut app,
            prompt(doc, GuardKind::DirtyClose),
            &mut crate::runtime::Effects::default()
        ),
        GuardRaise::Raised
    );

    assert!(
        app.search().is_some(),
        "a kept, unfocused search bar must outlive a guard raise"
    );
}

#[test]
fn a_focused_search_bar_closes_on_a_guard_raise() {
    let mut app = app();
    let doc = app.active;
    crate::search::open(&mut app, &mut crate::runtime::Effects::default());
    assert!(app.search().expect("the bar is open").focused);

    assert_eq!(
        set_guard(
            &mut app,
            prompt(doc, GuardKind::DirtyClose),
            &mut crate::runtime::Effects::default()
        ),
        GuardRaise::Raised
    );

    assert!(
        app.search().is_none(),
        "a focused bar must yield the keyboard"
    );
}
