//! Work package A: a document that already has a `file_path` AND a
//! create-only `DocDb` — a named file that does not exist on disk
//! yet, the shape a launch onto a not-yet-existing positional leaves
//! behind. `rename_common::unsaved_named_session` is the shared
//! fixture; the end-to-end tests here drive it through the same public
//! entry points a user reaches (⌘S, `^R`), through `rune_fuzz::Session` now
//! that `SAVE-INFLIGHT-SM` recognizes the title-focused Enter that commits
//! a `bind_new_now` create.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod rename_common;

use std::path::Path;
use std::sync::Arc;

use rune_tui::keymap::KeyCode;
use rune_vfs::{Mem, Vfs, VfsTestExt};

use rename_common::{
    UNPUBLISHED_BODY, active_path, commit_name, drain_materialize_round_trip, plain_key, sup_key,
    unsaved_named_session,
};

/// ⌘S on a named-but-unpublished document creates the file with the
/// buffer's exact bytes, flips the publish mode to overwrite, and leaves
/// the document clean.
#[test]
fn cmd_s_creates_the_file_and_flips_to_overwrite() {
    let mem = Arc::new(Mem::new());
    let mut session = unsaved_named_session(&mem);
    let id = session.app().active;

    assert!(session.key(sup_key('s')).is_none());
    drain_materialize_round_trip(&mut session);

    assert_eq!(
        mem.read(Path::new("/root/nope.md")).expect("file created"),
        UNPUBLISHED_BODY.as_bytes()
    );
    let doc_db = session
        .app()
        .doc(id)
        .unwrap()
        .doc_db()
        .expect("still bound");
    assert_eq!(
        doc_db.publish_mode,
        rune_tui::db::PublishMode::OverwriteExisting,
        "the create just committed — the next save is an overwrite"
    );
    assert!(
        !session.app().doc(id).unwrap().is_dirty(),
        "a just-published buffer must not read as dirty"
    );
}

/// ⌘S when the path is already occupied by other bytes (a create that
/// lost the race, A2): the other bytes survive untouched, one error is
/// posted, no `DiskConflict` guard is left standing, and the document
/// ends up bound to the file's own row in the overwrite publish mode.
#[test]
fn cmd_s_on_a_lost_create_race_leaves_the_racers_bytes_and_rebinds() {
    let mem = Arc::new(Mem::new());
    let mut session = unsaved_named_session(&mem);
    let id = session.app().active;
    // The racer wins AFTER this session's own fixture has already booted
    // (unaware of the path) — the concurrent write a "lost race" actually
    // names, and the order `unsaved_named_session` itself requires: it
    // opens directly on this path while absent, so seeding it first would
    // have the fixture's own boot observe (and open) the racer's file
    // instead of falling back to the create-only draft this test needs.
    mem.save_atomic(Path::new("/root/nope.md"), b"racer bytes")
        .expect("a concurrent creator wins first");

    assert!(session.key(sup_key('s')).is_none());
    drain_materialize_round_trip(&mut session);

    // The hand-off seeds `last_sync = Diverged` the instant the race is
    // detected — before the Load round trip even lands — so `^M` is a
    // genuine escape hatch during the window a slower writer thread would
    // otherwise leave the user stuck with only the plain error message.
    assert_eq!(
        session.app().doc(id).unwrap().last_sync,
        Some(rune_db::SyncKind::Diverged),
        "the A2 branch must seed Diverged so the merge route is reachable"
    );

    // The A2 route hands the document off to an ordinary Load to install a
    // real CAS baseline instead of raising an unanswerable Guard.
    assert!(session.deliver_db_all().is_none());

    // Review fix: the hand-off's Load is `binding_only` — its ack must
    // never clobber the `Diverged` seed with its own freshly-observed
    // (clean) sync kind, or `^M` stops being reachable the instant the
    // round trip lands.
    assert_eq!(
        session.app().doc(id).unwrap().last_sync,
        Some(rune_db::SyncKind::Diverged),
        "the hand-off's Load ack must never overwrite the Diverged seed"
    );

    assert_eq!(
        mem.read(Path::new("/root/nope.md")).expect("still there"),
        b"racer bytes",
        "the racer's bytes must survive untouched"
    );
    assert!(
        rune_tui::messages::newest_text(session.app())
            .is_some_and(|m| m.contains("created by something")),
        "got {:?}",
        rune_tui::messages::newest_text(session.app())
    );
    assert!(
        session.app().guard.is_none(),
        "a lost create race must never raise an unanswerable DiskConflict guard"
    );
    let doc_db = session
        .app()
        .doc(id)
        .unwrap()
        .doc_db()
        .expect("still bound");
    assert_eq!(
        doc_db.publish_mode,
        rune_tui::db::PublishMode::OverwriteExisting,
        "the document must come out of the create bound to the file's own row"
    );
    assert_eq!(
        session.app().doc(id).unwrap().buffer.content(),
        UNPUBLISHED_BODY,
        "the user's typed body must never be clobbered by the hand-off's Load"
    );
}

/// The hand-off must never fire when another live document in this session
/// is already bound to the very path the race collided on — that other
/// document's row may carry this-session history, which `hydrate` would
/// replace this buffer's typing with. Instead the refusal stays plain, with
/// the actionable message telling the user their buffer is intact and `^R`
/// is the way out, and the document stays create-only so a later ⌘S keeps retrying
/// create-only semantics rather than ever falling back to a direct-vfs
/// overwrite of a file this session has never observed.
#[test]
fn cmd_s_on_a_lost_create_race_already_open_elsewhere_keeps_the_plain_refusal() {
    let mem = Arc::new(Mem::new());
    let mut session = unsaved_named_session(&mem);
    let id = session.app().active;
    // See `cmd_s_on_a_lost_create_race_leaves_the_racers_bytes_and_rebinds`
    // for why the racer's write must land AFTER the fixture boots.
    mem.save_atomic(Path::new("/root/nope.md"), b"racer bytes")
        .expect("a concurrent creator wins first");

    // A second, unrelated live document already bound to the racer's path.
    let racer_path = rune_tui::resolved::ResolvedPath::resolve(
        session.app().vfs.as_ref(),
        Path::new("/root/nope.md"),
    )
    .expect("the racer's path resolves");
    session.app_mut().open_document_bound(
        rune_core::buffer::Buffer::new("other tab's body"),
        racer_path,
    );

    assert!(session.key(sup_key('s')).is_none());
    drain_materialize_round_trip(&mut session);

    assert!(
        rune_tui::messages::newest_text(session.app()).is_some_and(|m| m.contains("^R")),
        "got {:?}",
        rune_tui::messages::newest_text(session.app())
    );
    assert!(
        session.app().doc(id).unwrap().buffer.content() == UNPUBLISHED_BODY,
        "the buffer must stay exactly as typed"
    );
    let doc_db = session
        .app()
        .doc(id)
        .unwrap()
        .doc_db()
        .expect("still bound");
    assert!(
        doc_db.publish_mode.is_create_only(),
        "no hand-off happened, so the document must stay create-only"
    );
    assert!(
        !session
            .app()
            .db_ops
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
    let mut session = unsaved_named_session(&mem);

    // `rename::begin`'s create branch routes through `save::bind_new_now`,
    // the same store-backed materialize dance ⌘S uses.
    commit_name(&mut session, "fresh");
    drain_materialize_round_trip(&mut session);

    let path = active_path(session.app()).expect("the document must now be bound");
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
    let mut session = unsaved_named_session(&mem);
    let id = session.app().active;

    commit_name(&mut session, "taken");
    drain_materialize_round_trip(&mut session);

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
        !session
            .app()
            .db_ops
            .values()
            .any(|p| p.doc == id && p.issued_version.is_some()),
        "a rename-route EEXIST must never enqueue a Load for the old, never-existing name"
    );
    assert!(
        !session.app().db.as_ref().unwrap().degraded,
        "a plain rename collision must never degrade the store"
    );
    assert!(
        session.app().guard.is_none(),
        "a rename-create collision has no CAS baseline to raise a Guard against"
    );
    let doc_db = session
        .app()
        .doc(id)
        .unwrap()
        .doc_db()
        .expect("still bound");
    assert!(
        doc_db.publish_mode.is_create_only(),
        "the document must stay create-only — no naming attempt has succeeded yet"
    );
    assert!(
        session.app().doc(id).unwrap().bind_target().is_none(),
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
    let mut session = unsaved_named_session(&mem);
    let id = session.app().active;

    // First attempt: collides, refused.
    commit_name(&mut session, "taken");
    drain_materialize_round_trip(&mut session);

    // The refusal returns focus to the title, still holding the refused
    // name — `Esc` leaves it without re-committing, exactly like any other
    // manual cancel, so the second attempt below is a genuine plain save,
    // never a second attempt at the same refused rename.
    assert!(session.key(plain_key(KeyCode::Escape)).is_none());

    // Second attempt: a plain save of the document's OWN path.
    assert!(session.key(sup_key('s')).is_none());
    drain_materialize_round_trip(&mut session);

    let path = active_path(session.app()).expect("the document must now be bound");
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
    let doc_db = session
        .app()
        .doc(id)
        .unwrap()
        .doc_db()
        .expect("still bound");
    assert_eq!(
        doc_db.publish_mode,
        rune_tui::db::PublishMode::OverwriteExisting,
        "the create just committed"
    );
}

/// Blocker 3 regression (moved out of `materialize_ack::reactions`'s own
/// internal test module, A2 — the observable property is "the next ⌘S
/// still writes the file", not the document's replica internal shape): `record_outcome`'s
/// "the store vanished entirely mid-flight" synthetic-commit arm builds a
/// `MatResult::Committed { saved: None }`.
/// With no store left to re-baseline from, the document's binding must be
/// dropped rather than left standing with a stale `expect_obs` that would
/// make the very next save's `materialize_prepare` immediately `NotFound`.
/// Simulated by dropping `app.db` between the caller-side `vfs` write
/// committing and its `MaterializeRecord` bookkeeping landing — the exact
/// window `record_outcome`'s doc comment describes. `app.db` is nulled
/// directly through `app_mut()` between the `MaterializePrepare` ack (which
/// parks the caller-side vfs `Cmd` as the driver's own pending save) and
/// its delivery — a plain state poke, not a fabricated message, so the
/// checked steps around it stay real.
#[test]
fn a_synthesized_commit_with_no_store_left_drops_the_binding_and_the_next_save_still_lands() {
    let mem = Arc::new(Mem::new());
    let mut session = unsaved_named_session(&mem);
    let id = session.app().active;

    assert!(session.key(sup_key('s')).is_none());
    assert!(session.deliver_db().is_none());

    // The store vanishes entirely mid-flight, after the write already
    // committed but before its bookkeeping lands.
    session.app_mut().db = None;
    assert!(session.deliver().is_none());

    assert_eq!(
        mem.read(Path::new("/root/nope.md")).expect("file created"),
        UNPUBLISHED_BODY.as_bytes(),
        "the write itself already committed before the store vanished"
    );
    assert!(
        !session.app().doc(id).unwrap().is_dirty(),
        "the just-published bytes must still count as saved"
    );
    assert!(
        !session.app().doc(id).unwrap().is_store_bound(),
        "a binding that can never serve its next save must be dropped, not left dangling"
    );

    // A second save, now routed through the no-store direct-vfs fallback,
    // must still land — the Prime Directive holds even once the binding
    // is gone.
    assert!(session.type_("!").is_none());
    assert!(session.key(sup_key('s')).is_none());
    assert!(session.deliver().is_none());

    assert_eq!(
        mem.read(Path::new("/root/nope.md")).unwrap(),
        format!("{UNPUBLISHED_BODY}!").as_bytes(),
        "the second save must still reach disk despite the dropped binding"
    );
    assert!(!session.app().doc(id).unwrap().is_dirty());
}
