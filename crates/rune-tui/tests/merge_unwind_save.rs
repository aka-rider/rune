//! Issue #65: undoing back past a completed merge leaves a buffer that no
//! longer descends from what is on disk, while the CAS baseline still
//! matches those exact disk bytes — the one shape where the
//! compare-and-swap agrees and only the store's own classification can tell
//! the two cases apart. Whose bytes are on disk decides: bytes rune
//! published are the user's own and overwriting them is an ordinary save;
//! bytes an external program put there were only ever ADOPTED into the
//! buffer, and the undo withdraws that adoption. Driven through
//! `rune_fuzz::Session`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod merge_common;

use std::path::Path;

use rune_db::SyncKind;
use rune_fuzz::Session;
use rune_tui::document::DocumentId;
use rune_tui::guard::GuardKind;
use rune_tui::keymap::KeyCode;
use rune_tui::merge::MergeState;

use merge_common::{
    bare, ch, ctrl, drain_materialize_round_trip, external_write, reprobe, save_and_ack,
    save_expecting_refusal, sup, untitled_draft,
};

const ANCESTOR: &str = "one\ntwo\nthree\nfour\nfive\n";
const THEIRS: &[u8] = b"one disk\ntwo\nthree\nfour\nfive disk\n";
/// The buffer `^M` is entered from: line 1 edited by the user, line 5 too.
const BEFORE_MERGE: &str = "Xone\ntwo\nthree\nfour\nfiveZ\n";
/// What resolving `[O]urs` then `[T]heirs` leaves in the buffer: the first
/// block keeps the editor's own line, the second takes disk's.
const AFTER_MERGE: &str = "Xone\ntwo\nthree\nfour\nfive disk\n";

struct Reconciled {
    session: Session,
    doc: DocumentId,
    pre_merge_journal_pos: usize,
}

impl Reconciled {
    fn buffer(&self) -> String {
        self.session
            .app()
            .doc(self.doc)
            .unwrap()
            .buffer
            .content()
            .to_string()
    }

    fn on_disk(&self) -> Vec<u8> {
        self.session.app().vfs.read(Path::new("/doc.md")).unwrap()
    }

    fn journal_pos(&self) -> usize {
        self.session.app().doc(self.doc).unwrap().journal.pos()
    }
}

fn ancestor_line_count() -> usize {
    ANCESTOR.bytes().filter(|&byte| byte == b'\n').count()
}

/// Opens `/doc.md` seeded with `ANCESTOR`, edits both conflict lines into
/// `BEFORE_MERGE`, rewrites the disk to `THEIRS` behind the editor's back,
/// reprobes into `Diverged`, and resolves every block — stopping short of
/// any save, so disk still holds the external bytes the merge only ever
/// adopted INTO the buffer.
fn merged() -> Reconciled {
    let mut session = Session::open("/doc.md", ANCESTOR);
    let doc = session.app().active;
    let draft_id = untitled_draft(session.app(), doc);

    assert!(session.key(ch('X')).is_none());
    for _ in 0..ancestor_line_count() - 1 {
        assert!(session.key(bare(KeyCode::Down)).is_none());
    }
    assert!(session.key(bare(KeyCode::End)).is_none());
    assert!(session.key(ch('Z')).is_none());
    assert!(session.deliver_db_all().is_none());
    assert_eq!(
        session.app().doc(doc).unwrap().buffer.content(),
        BEFORE_MERGE
    );
    let pre_merge_journal_pos = session.app().doc(doc).unwrap().journal.pos();

    external_write(session.app().vfs.as_ref(), THEIRS);
    reprobe(&mut session, draft_id, doc);
    assert_eq!(
        session.app().doc(doc).unwrap().last_sync,
        Some(SyncKind::Diverged)
    );

    assert!(session.key(ctrl('m')).is_none());
    assert!(session.deliver_db().is_none());
    assert!(matches!(session.app().merge, MergeState::Active { .. }));
    assert!(session.key(ch('o')).is_none());
    assert!(session.key(ch('t')).is_none());
    assert_eq!(session.app().merge, MergeState::Inactive);
    assert!(session.deliver_db_all().is_none());

    let fixture = Reconciled {
        session,
        doc,
        pre_merge_journal_pos,
    };
    assert_eq!(fixture.buffer(), AFTER_MERGE);
    assert_eq!(fixture.on_disk(), THEIRS);
    fixture
}

/// [`merged`] plus the ⌘S the merge invites: rune itself now published the
/// merged bytes, so disk carries the user's own hand and nobody else's.
fn merged_and_published() -> Reconciled {
    let mut fixture = merged();
    save_and_ack(&mut fixture.session);
    assert!(
        fixture.session.app().guard.is_none(),
        "the invited post-merge save must commit"
    );
    assert_eq!(fixture.on_disk(), AFTER_MERGE.as_bytes());
    fixture
}

/// `⌘Z` all the way back over the merge — every resolution AND the working
/// form's own install — leaving the buffer exactly where it stood before
/// `^M`, delivering each undo's journal op as the real runtime would. The
/// journal entries the merge itself added bound the press count; returns how
/// many it actually took, so the redo counterpart needs no bound of its own.
fn undo_past_the_merge(fixture: &mut Reconciled) -> usize {
    let bound = fixture.journal_pos() - fixture.pre_merge_journal_pos;
    let mut presses = 0;
    while fixture.journal_pos() > fixture.pre_merge_journal_pos && presses < bound {
        assert!(fixture.session.key(sup('z')).is_none());
        assert!(fixture.session.deliver_db_all().is_none());
        presses += 1;
    }
    assert_eq!(
        fixture.buffer(),
        BEFORE_MERGE,
        "the merge never unwound in {bound} undo(s)"
    );
    presses
}

/// The exact counterpart: `^Y` back over everything [`undo_past_the_merge`]
/// unwound, one press per undo it took.
fn redo_back_over_the_merge(fixture: &mut Reconciled, undos: usize) {
    for _ in 0..undos {
        assert!(fixture.session.key(ctrl('y')).is_none());
        assert!(fixture.session.deliver_db_all().is_none());
    }
    assert_eq!(
        fixture.buffer(),
        AFTER_MERGE,
        "the redo must restore every resolution in {undos} press(es)"
    );
}

/// A user cannot conflict with their own changes. Disk holds bytes rune
/// itself published, so undoing back behind them and saving is an ordinary
/// save: no Guard, no divergence marker, and the pre-merge buffer reaches
/// disk.
#[test]
fn save_after_undo_past_a_published_merge_writes() {
    let mut fixture = merged_and_published();
    undo_past_the_merge(&mut fixture);
    assert!(
        !fixture
            .session
            .app()
            .doc(fixture.doc)
            .unwrap()
            .last_sync
            .is_some_and(SyncKind::is_disk_divergent),
        "our own published bytes must never read as a divergence, log: {:?}",
        rune_tui::messages::log_text(fixture.session.app())
    );

    save_and_ack(&mut fixture.session);

    assert!(
        fixture.session.app().guard.is_none(),
        "overwriting bytes rune itself published needs no confirmation, log: {:?}",
        rune_tui::messages::log_text(fixture.session.app())
    );
    assert_eq!(
        fixture.on_disk(),
        BEFORE_MERGE.as_bytes(),
        "the undone buffer must reach disk"
    );
    fixture.session.app_mut().recompute_dirty(fixture.doc);
    assert!(!fixture.session.app().doc(fixture.doc).unwrap().is_dirty());
}

/// The mirror case: with no intermediate save, disk still holds the
/// EXTERNAL bytes the merge only adopted into the buffer. The CAS baseline
/// matches them exactly, so the compare-and-swap would happily publish a
/// buffer undone back behind everything the merge brought in. The undo
/// withdrew that adoption, so the save is refused before any `vfs` call.
#[test]
fn save_after_undo_past_an_unpublished_merge_is_refused() {
    let mut fixture = merged();
    undo_past_the_merge(&mut fixture);

    save_expecting_refusal(&mut fixture.session);

    let Some(prompt) = &fixture.session.app().guard else {
        panic!(
            "expected the disk-conflict Guard, not a silent overwrite, log: {:?}",
            rune_tui::messages::log_text(fixture.session.app())
        );
    };
    assert_eq!(prompt.doc, fixture.doc);
    assert!(matches!(prompt.kind, GuardKind::DiskConflict));
    assert_eq!(
        fixture.on_disk(),
        THEIRS,
        "the refused save must leave the external bytes on disk untouched"
    );
    assert_eq!(
        fixture.buffer(),
        BEFORE_MERGE,
        "a refused save never touches the buffer"
    );
    fixture.session.app_mut().recompute_dirty(fixture.doc);
    assert!(
        fixture.session.app().doc(fixture.doc).unwrap().is_dirty(),
        "a refused save must never mark the document saved"
    );
    assert!(
        !fixture
            .session
            .app()
            .doc(fixture.doc)
            .unwrap()
            .save_in_flight(),
        "the refused attempt must be resolved, not left in flight"
    );
}

/// The escape hatch still works: `[S]ave anyway` forces past the gate the
/// same way it forces past the CAS, and the buffer reaches disk.
#[test]
fn save_anyway_after_the_refusal_publishes_the_buffer() {
    let mut fixture = merged();
    undo_past_the_merge(&mut fixture);
    save_expecting_refusal(&mut fixture.session);

    assert!(fixture.session.key(ch('s')).is_none());
    drain_materialize_round_trip(&mut fixture.session);

    assert!(
        fixture.session.app().guard.is_none(),
        "a force-save must never re-raise the conflict it was answering"
    );
    assert_eq!(
        fixture.on_disk(),
        BEFORE_MERGE.as_bytes(),
        "[S]ave anyway must publish the buffer's own bytes"
    );
    fixture.session.app_mut().recompute_dirty(fixture.doc);
    assert!(!fixture.session.app().doc(fixture.doc).unwrap().is_dirty());
}

/// The control for the refusal case below: `^w` answered with `[S]ave`
/// arms a close on that save, and a save that commits does close the
/// document.
#[test]
fn close_on_save_fires_when_the_save_commits() {
    let mut fixture = merged_and_published();
    undo_past_the_merge(&mut fixture);

    assert!(fixture.session.key(ctrl('w')).is_none());
    assert!(fixture.session.key(ch('s')).is_none());
    drain_materialize_round_trip(&mut fixture.session);

    assert_eq!(fixture.on_disk(), BEFORE_MERGE.as_bytes());
    assert!(
        fixture.session.app().doc(fixture.doc).is_none(),
        "a committed save-and-close must close the document, log: {:?}",
        rune_tui::messages::log_text(fixture.session.app())
    );
}

/// The same `^w` → `[S]ave`, refused by the gate instead: the close intent
/// dies with the attempt it was riding on. The document stays open, and the
/// `[S]ave anyway` the user answers with publishes without closing anything
/// out from under them.
#[test]
fn a_refused_save_never_leaves_a_close_armed_for_the_next_one() {
    let mut fixture = merged();
    undo_past_the_merge(&mut fixture);

    assert!(fixture.session.key(ctrl('w')).is_none());
    assert!(fixture.session.key(ch('s')).is_none());
    assert!(fixture.session.deliver_db().is_none());

    assert!(
        fixture.session.app().doc(fixture.doc).is_some(),
        "a refused save must never close the document, log: {:?}",
        rune_tui::messages::log_text(fixture.session.app())
    );

    assert!(fixture.session.key(ch('s')).is_none());
    drain_materialize_round_trip(&mut fixture.session);

    assert_eq!(
        fixture.on_disk(),
        BEFORE_MERGE.as_bytes(),
        "[S]ave anyway must still publish the buffer's own bytes"
    );
    assert!(
        fixture.session.app().doc(fixture.doc).is_some(),
        "the close the refusal dropped must never fire on a later save"
    );
}

/// The refusal is about where the buffer stands, not a sticky flag: redoing
/// back over the merge and typing on restores an ordinary save that commits
/// with no Guard at all.
#[test]
fn redo_back_over_the_merge_restores_an_ordinary_save() {
    let mut fixture = merged();
    let undos = undo_past_the_merge(&mut fixture);
    save_expecting_refusal(&mut fixture.session);

    assert!(fixture.session.key(bare(KeyCode::Escape)).is_none());
    assert!(fixture.session.app().guard.is_none());
    redo_back_over_the_merge(&mut fixture, undos);
    assert!(fixture.session.key(ch('!')).is_none());
    assert!(fixture.session.deliver_db_all().is_none());
    let redone = fixture.buffer();

    save_and_ack(&mut fixture.session);

    assert!(
        fixture.session.app().guard.is_none(),
        "the redone merge form must save without a Guard, log: {:?}",
        rune_tui::messages::log_text(fixture.session.app())
    );
    assert_eq!(fixture.on_disk(), redone.as_bytes());
    fixture.session.app_mut().recompute_dirty(fixture.doc);
    assert!(!fixture.session.app().doc(fixture.doc).unwrap().is_dirty());
}
