#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod merge_common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rune_fuzz::Session;
use rune_tui::db::Db;
use rune_tui::guard::GuardKind;
use rune_tui::merge::MergeState;
use rune_vfs::{Mem, Vfs};

use merge_common::db_wiring_common::{publish, store_at, temp_db_dir};
use merge_common::{ch, save_and_ack};

fn temp_db_path(label: &str) -> PathBuf {
    temp_db_dir(&format!("merge-second-instance-{label}")).join("rune-db.sqlite")
}

fn open_instance(db_path: &Path, mem: &Arc<Mem>) -> Session {
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::clone(mem) as Arc<dyn Vfs + Send + Sync>;
    let (store, bridge) = store_at(db_path, vfs);
    Session::open_with_db("/doc.md", Arc::clone(mem), Db::new(store, bridge, false))
}

#[test]
fn second_instances_save_invites_a_real_merge_not_a_silent_loop() {
    let db_path = temp_db_path("loop");
    let mem = Arc::new(Mem::new());
    publish(mem.as_ref(), Path::new("/doc.md"), b"base");

    let mut instance1 = open_instance(&db_path, &mem);
    let mut instance2 = open_instance(&db_path, &mem);

    assert!(instance2.key(ch('B')).is_none());
    assert!(instance2.deliver_db().is_none());
    save_and_ack(&mut instance2);
    assert!(
        instance2.app().guard.is_none(),
        "test setup: instance 2's own first save must not conflict"
    );
    assert_eq!(
        mem.read(Path::new("/doc.md")).unwrap(),
        b"Bbase",
        "test setup: instance 2's save must have reached disk"
    );

    assert!(instance1.key(ch('A')).is_none());
    assert!(instance1.deliver_db().is_none());
    save_and_ack(&mut instance1);
    let Some(prompt) = &instance1.app().guard else {
        panic!("expected instance 1's own save to CAS-refuse into the disk-conflict Guard");
    };
    assert!(matches!(prompt.kind, GuardKind::DiskConflict));

    assert!(instance1.key(ch('m')).is_none());
    assert!(instance1.app().guard.is_none());
    assert!(instance1.deliver_db_all().is_none());

    assert!(
        matches!(instance1.app().merge, MergeState::Active { .. }),
        "instance 1's merge must become Active against instance 2's save, got {:?}",
        instance1.app().merge
    );
}
