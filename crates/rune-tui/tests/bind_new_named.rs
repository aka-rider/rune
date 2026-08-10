//! Work package A: a document that already has a `file_path` AND a
//! `DocDb { bind_new: true }` — a named file that does not exist on disk
//! yet, the shape a launch onto a not-yet-existing positional leaves
//! behind. `rename_common::unsaved_named_app_with_store` is the shared
//! fixture; the end-to-end tests here drive it through the same public
//! entry points a user reaches (⌘S, `^R`).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod rename_common;

use std::path::Path;
use std::sync::Arc;

use rune_tui::runtime::{CmdKind, Msg};

use rune_vfs::{Mem, Vfs};

use rename_common::{
    UNPUBLISHED_BODY, active_path, rename_to, send, sup, type_text, wait_for_load,
    wait_for_materialize_prep, wait_for_materialize_record,
};

/// ⌘S on a named-but-unpublished document creates the file with the
/// buffer's exact bytes, clears `bind_new`, and leaves the document clean.
#[test]
fn cmd_s_creates_the_file_and_clears_bind_new() {
    let mem = Arc::new(Mem::new());
    let (mut app, bridge) = rename_common::unsaved_named_app_with_store(&mem);
    let id = app.active;

    send(&mut app, sup('s'));

    let prep_evt = wait_for_materialize_prep(&bridge);
    let mut effects = send(&mut app, Msg::Db(prep_evt));
    let cmd = effects
        .cmds
        .drain(..)
        .find(|c| c.kind() == CmdKind::Save)
        .expect("the prepare ack must spawn the caller-side vfs Cmd");
    let vfs_done = cmd.run().expect("the vfs Cmd must reply");
    send(&mut app, vfs_done);

    let record_evt = wait_for_materialize_record(&bridge);
    send(&mut app, Msg::Db(record_evt));

    assert_eq!(
        mem.read(Path::new("/root/nope.md")).expect("file created"),
        UNPUBLISHED_BODY.as_bytes()
    );
    let doc_db = app.doc(id).unwrap().doc_db().expect("still bound");
    assert!(
        !doc_db.bind_new,
        "the create just committed — the next save is an overwrite"
    );
    assert!(
        !app.doc(id).unwrap().is_dirty(),
        "a just-published buffer must not read as dirty"
    );
}

/// ⌘S when the path is already occupied by other bytes (a create that
/// lost the race, A2): the other bytes survive untouched, one error is
/// posted, no `DiskConflict` guard is left standing, and the document
/// ends up bound to the file's own row with `bind_new == false`.
#[test]
fn cmd_s_on_a_lost_create_race_leaves_the_racers_bytes_and_rebinds() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(Path::new("/root/nope.md"), b"racer bytes")
        .expect("a concurrent creator wins first");
    let (mut app, bridge) = rename_common::unsaved_named_app_with_store(&mem);
    let id = app.active;

    send(&mut app, sup('s'));

    let prep_evt = wait_for_materialize_prep(&bridge);
    let mut effects = send(&mut app, Msg::Db(prep_evt));
    let cmd = effects
        .cmds
        .drain(..)
        .find(|c| c.kind() == CmdKind::Save)
        .expect("the prepare ack must spawn the caller-side vfs Cmd");
    let vfs_done = cmd.run().expect("the vfs Cmd must reply");
    send(&mut app, vfs_done);

    let record_evt = wait_for_materialize_record(&bridge);
    send(&mut app, Msg::Db(record_evt));

    // The hand-off seeds `last_sync = Diverged` the instant the race is
    // detected — before the Load round trip even lands — so `^M` is a
    // genuine escape hatch during the window a slower writer thread would
    // otherwise leave the user stuck with only the plain error message.
    assert_eq!(
        app.doc(id).unwrap().last_sync,
        Some(rune_db::SyncKind::Diverged),
        "the A2 branch must seed Diverged so the merge route is reachable"
    );

    // The A2 route hands the document off to an ordinary Load to install a
    // real CAS baseline instead of raising an unanswerable Guard.
    let load_evt = wait_for_load(&bridge);
    send(&mut app, Msg::Db(load_evt));

    // Review fix: the hand-off's Load is `binding_only` — its ack must
    // never clobber the `Diverged` seed with its own freshly-observed
    // (clean) sync kind, or `^M` stops being reachable the instant the
    // round trip lands.
    assert_eq!(
        app.doc(id).unwrap().last_sync,
        Some(rune_db::SyncKind::Diverged),
        "the hand-off's Load ack must never overwrite the Diverged seed"
    );

    assert_eq!(
        mem.read(Path::new("/root/nope.md")).expect("still there"),
        b"racer bytes",
        "the racer's bytes must survive untouched"
    );
    assert!(
        rune_tui::messages::newest_text(&app).is_some_and(|m| m.contains("created by something")),
        "got {:?}",
        rune_tui::messages::newest_text(&app)
    );
    assert!(
        app.guard.is_none(),
        "a lost create race must never raise an unanswerable DiskConflict guard"
    );
    let doc_db = app.doc(id).unwrap().doc_db().expect("still bound");
    assert!(
        !doc_db.bind_new,
        "the document must come out of bind_new bound to the file's own row"
    );
    assert_eq!(
        app.doc(id).unwrap().buffer.content(),
        UNPUBLISHED_BODY,
        "the user's typed body must never be clobbered by the hand-off's Load"
    );
}

/// The hand-off must never fire when another live document in this session
/// is already bound to the very path the race collided on — that other
/// document's row may carry this-session history, which `hydrate` would
/// replace this buffer's typing with. Instead the refusal stays plain, with
/// the actionable message telling the user their buffer is intact and `^R`
/// is the way out, and `bind_new` stays `true` so a later ⌘S keeps retrying
/// create-only semantics rather than ever falling back to a direct-vfs
/// overwrite of a file this session has never observed.
#[test]
fn cmd_s_on_a_lost_create_race_already_open_elsewhere_keeps_the_plain_refusal() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(Path::new("/root/nope.md"), b"racer bytes")
        .expect("a concurrent creator wins first");
    let (mut app, bridge) = rename_common::unsaved_named_app_with_store(&mem);
    let id = app.active;

    // A second, unrelated live document already bound to the racer's path.
    let other = app.open_document(rune_core::buffer::Buffer::new("other tab's body"));
    app.doc_mut(other).unwrap().file_path = Some(std::path::PathBuf::from("/root/nope.md"));

    send(&mut app, sup('s'));

    let prep_evt = wait_for_materialize_prep(&bridge);
    let mut effects = send(&mut app, Msg::Db(prep_evt));
    let cmd = effects
        .cmds
        .drain(..)
        .find(|c| c.kind() == CmdKind::Save)
        .expect("the prepare ack must spawn the caller-side vfs Cmd");
    let vfs_done = cmd.run().expect("the vfs Cmd must reply");
    send(&mut app, vfs_done);

    let record_evt = wait_for_materialize_record(&bridge);
    send(&mut app, Msg::Db(record_evt));

    assert!(
        rune_tui::messages::newest_text(&app).is_some_and(|m| m.contains("^R")),
        "got {:?}",
        rune_tui::messages::newest_text(&app)
    );
    assert!(
        app.doc(id).unwrap().buffer.content() == UNPUBLISHED_BODY,
        "the buffer must stay exactly as typed"
    );
    let doc_db = app.doc(id).unwrap().doc_db().expect("still bound");
    assert!(
        doc_db.bind_new,
        "no hand-off happened, so bind_new must stay true"
    );
    assert!(
        !app.db_ops
            .values()
            .any(|p| p.doc == id && p.issued_version.is_some()),
        "no Load must have been enqueued for the colliding document"
    );
}

/// `^R` + a new name on a never-published document creates the new name
/// in the document's OWN directory (A3), and the old name is never
/// created.
#[test]
fn rename_on_a_never_published_document_creates_at_the_new_name() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(Path::new("/fresh.md"), b"unrelated")
        .expect(
            "seed the root-joined target so a wrongly root-joined create would actually collide",
        );
    let (mut app, bridge) = rename_common::unsaved_named_app_with_store(&mem);

    rename_to(&mut app, "fresh");

    // `rename::begin`'s create branch routes through `save::bind_new_now`,
    // the same store-backed materialize dance ⌘S uses: a `MaterializePrepare`
    // ack spawns the caller-side `vfs` `Cmd`, which itself replies with a
    // `Msg` that enqueues `MaterializeRecord`.
    let prep_evt = wait_for_materialize_prep(&bridge);
    let mut effects = send(&mut app, Msg::Db(prep_evt));
    let cmd = effects
        .cmds
        .drain(..)
        .find(|c| c.kind() == CmdKind::Save)
        .expect("the prepare ack must spawn the caller-side vfs Cmd");
    let vfs_done = cmd.run().expect("the vfs Cmd must reply");
    send(&mut app, vfs_done);

    let record_evt = wait_for_materialize_record(&bridge);
    send(&mut app, Msg::Db(record_evt));

    let path = active_path(&app).expect("the document must now be bound");
    assert_eq!(
        path,
        Path::new("/root/fresh.md"),
        "must create next to the document's own directory, not the workspace root"
    );
    assert_eq!(mem.read(&path).unwrap(), UNPUBLISHED_BODY.as_bytes());
    assert!(
        mem.read(Path::new("/root/nope.md")).is_err(),
        "the old, never-published name must never be created"
    );
}

/// `^R` + a new name that ALREADY exists (A3's own lost-create-race): the
/// EEXIST refusal must never route through the `lost_create_race`
/// hand-off — `bind_new_now` deliberately leaves `file_path` at the OLD,
/// never-published name (`/root/nope.md`) until the publish commits, so a
/// hand-off keyed off `file_path` would enqueue a `Load` for a file that
/// has never existed. The refusal must stay plain, no `Load` enqueued, no
/// store degrade, and the old name stays uncreated.
#[test]
fn rename_to_an_existing_name_never_hands_off_to_load() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(Path::new("/root/taken.md"), b"already here")
        .expect("a file already sits at the rename target");
    let (mut app, bridge) = rename_common::unsaved_named_app_with_store(&mem);
    let id = app.active;

    rename_to(&mut app, "taken");

    let prep_evt = wait_for_materialize_prep(&bridge);
    let mut effects = send(&mut app, Msg::Db(prep_evt));
    let cmd = effects
        .cmds
        .drain(..)
        .find(|c| c.kind() == CmdKind::Save)
        .expect("the prepare ack must spawn the caller-side vfs Cmd");
    let vfs_done = cmd.run().expect("the vfs Cmd must reply");
    send(&mut app, vfs_done);

    let record_evt = wait_for_materialize_record(&bridge);
    send(&mut app, Msg::Db(record_evt));

    assert_eq!(
        mem.read(Path::new("/root/taken.md")).expect("still there"),
        b"already here",
        "the existing file must survive untouched"
    );
    assert!(
        mem.read(Path::new("/root/nope.md")).is_err(),
        "the never-published old name must never be created by the refused attempt"
    );
    assert!(
        !app.db_ops
            .values()
            .any(|p| p.doc == id && p.issued_version.is_some()),
        "a rename-route EEXIST must never enqueue a Load for the old, never-existing name"
    );
    assert!(
        !app.db.as_ref().unwrap().degraded,
        "a plain rename collision must never degrade the store"
    );
    assert!(
        app.guard.is_none(),
        "a rename-create collision has no CAS baseline to raise a Guard against"
    );
    let doc_db = app.doc(id).unwrap().doc_db().expect("still bound");
    assert!(
        doc_db.bind_new,
        "the document must stay bind_new — no naming attempt has succeeded yet"
    );
    assert!(
        app.doc(id).unwrap().bind_target().is_none(),
        "a refused create must clear bind_target"
    );
}

/// A refused create's `bind_target` must never survive to bind a LATER,
/// unrelated successful create. `^R` into a collision, refused, THEN a
/// plain ⌘S — never a second `^R`, which would route through
/// `bind_new_now` and unconditionally overwrite `bind_target` before the
/// commit ever consumes the stale one, masking the leak this test exists
/// to catch. ⌘S instead goes through `materialize_now`, which never
/// touches `bind_target` at all: if the first refusal left it standing,
/// the commit below would bind this document to the REFUSED name
/// (`taken.md`) while the bytes it actually just wrote landed at the
/// document's own, never-touched path (`nope.md`).
#[test]
fn a_refused_rename_create_never_leaks_its_path_into_a_later_successful_one() {
    let mem = Arc::new(Mem::new());
    mem.save_atomic(Path::new("/root/taken.md"), b"already here")
        .expect("a file already sits at the first rename target");
    let (mut app, bridge) = rename_common::unsaved_named_app_with_store(&mem);
    let id = app.active;

    // First attempt: collides, refused.
    rename_to(&mut app, "taken");
    let prep_evt = wait_for_materialize_prep(&bridge);
    let mut effects = send(&mut app, Msg::Db(prep_evt));
    let cmd = effects
        .cmds
        .drain(..)
        .find(|c| c.kind() == CmdKind::Save)
        .expect("the prepare ack must spawn the caller-side vfs Cmd");
    let vfs_done = cmd.run().expect("the vfs Cmd must reply");
    send(&mut app, vfs_done);
    let record_evt = wait_for_materialize_record(&bridge);
    send(&mut app, Msg::Db(record_evt));

    // The refusal returns focus to the title, still holding the refused
    // name — `Esc` leaves it without re-committing, exactly like any other
    // manual cancel, so the second attempt below is a genuine plain save,
    // never a second attempt at the same refused rename.
    send(
        &mut app,
        rename_common::plain(rune_tui::keymap::KeyCode::Escape),
    );

    // Second attempt: a plain save of the document's OWN path.
    send(&mut app, sup('s'));
    let prep_evt = wait_for_materialize_prep(&bridge);
    let mut effects = send(&mut app, Msg::Db(prep_evt));
    let cmd = effects
        .cmds
        .drain(..)
        .find(|c| c.kind() == CmdKind::Save)
        .expect("the prepare ack must spawn the caller-side vfs Cmd");
    let vfs_done = cmd.run().expect("the vfs Cmd must reply");
    send(&mut app, vfs_done);
    let record_evt = wait_for_materialize_record(&bridge);
    send(&mut app, Msg::Db(record_evt));

    let path = active_path(&app).expect("the document must now be bound");
    assert_eq!(
        path,
        Path::new("/root/nope.md"),
        "must bind to the document's OWN path — the one the bytes actually went to — \
         never the first, refused rename target"
    );
    assert_eq!(mem.read(&path).unwrap(), UNPUBLISHED_BODY.as_bytes());
    assert_eq!(
        mem.read(Path::new("/root/taken.md")).unwrap(),
        b"already here",
        "the refused target must stay exactly as it was"
    );
    let doc_db = app.doc(id).unwrap().doc_db().expect("still bound");
    assert!(!doc_db.bind_new, "the create just committed");
}

/// Blocker 3 regression (moved out of `materialize_ack::reactions`'s own
/// internal test module, A2 — the observable property is "the next ⌘S
/// still writes the file", not the document's replica internal shape): `record_outcome`'s
/// "the store vanished entirely mid-flight" synthetic-commit arm builds a
/// `MatResult { committed: true, ..Default::default() }` — `saved: None`.
/// With no store left to re-baseline from, the document's binding must be
/// dropped rather than left standing with a stale `expect_obs` that would
/// make the very next save's `materialize_prepare` immediately `NotFound`.
/// Simulated by dropping `app.db` between the caller-side `vfs` write
/// committing and its `MaterializeRecord` bookkeeping landing — the exact
/// window `record_outcome`'s doc comment describes.
#[test]
fn a_synthesized_commit_with_no_store_left_drops_the_binding_and_the_next_save_still_lands() {
    let mem = Arc::new(Mem::new());
    let (mut app, bridge) = rename_common::unsaved_named_app_with_store(&mem);
    let id = app.active;

    send(&mut app, sup('s'));
    let prep_evt = wait_for_materialize_prep(&bridge);
    let mut effects = send(&mut app, Msg::Db(prep_evt));
    let cmd = effects
        .cmds
        .drain(..)
        .find(|c| c.kind() == CmdKind::Save)
        .expect("the prepare ack must spawn the caller-side vfs Cmd");
    let vfs_done = cmd.run().expect("the vfs Cmd must reply");

    // The store vanishes entirely mid-flight, after the write already
    // committed but before its bookkeeping lands.
    app.db = None;
    send(&mut app, vfs_done);

    assert_eq!(
        mem.read(Path::new("/root/nope.md")).expect("file created"),
        UNPUBLISHED_BODY.as_bytes(),
        "the write itself already committed before the store vanished"
    );
    assert!(
        !app.doc(id).unwrap().is_dirty(),
        "the just-published bytes must still count as saved"
    );
    assert!(
        !app.doc(id).unwrap().is_store_bound(),
        "a binding that can never serve its next save must be dropped, not left dangling"
    );

    // A second save, now routed through the no-store direct-vfs fallback,
    // must still land — the Prime Directive holds even once the binding
    // is gone.
    type_text(&mut app, "!");
    let mut effects = send(&mut app, sup('s'));
    let save_cmd = effects
        .cmds
        .drain(..)
        .find(|c| c.kind() == CmdKind::Save)
        .expect("a dropped binding must fall back to the no-store save Cmd");
    let save_done = save_cmd.run().expect("the save Cmd must reply");
    send(&mut app, save_done);

    assert_eq!(
        mem.read(Path::new("/root/nope.md")).unwrap(),
        format!("{UNPUBLISHED_BODY}!").as_bytes(),
        "the second save must still reach disk despite the dropped binding"
    );
    assert!(!app.doc(id).unwrap().is_dirty());
}
