//! Post-merge sync regression suite (plan "merge-ux" Step 7): the
//! infinite re-prompt loop is dead. External divergence raises the ⇄
//! affordances, completing a merge retires them, a reprobe with an
//! untouched disk must NOT re-classify `Diverged` (the loop-killer), the
//! invited ⌘S commits without a CAS refusal, and a genuinely fresh second
//! external write still CAS-refuses into the disk-conflict Guard. Driven
//! through `rune_fuzz::Session`, pulling shared fixtures from
//! `merge_common`.
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
use rune_tui::app::App;
use rune_tui::document::DocumentId;
use rune_tui::footer;
use rune_tui::guard::GuardKind;
use rune_tui::keymap::KeyCode;
use rune_tui::merge::MergeState;
use rune_tui::testgrid;

use merge_common::{
    bare, ch, ctrl, drain_materialize_round_trip_unchecked, external_write, reprobe, save_and_ack,
    save_expecting_refusal, sup, take_ours, take_theirs, untitled_draft,
};

/// Both sides edit line 1 AND line 5 differently, with three untouched
/// context lines between — two separate conflicts under any diff engine
/// (the same spacing `merge_resolver.rs`'s fixture uses).
const ANCESTOR: &str = "one\ntwo\nthree\nfour\nfive\n";
const THEIRS: &[u8] = b"one disk\ntwo\nthree\nfour\nfive disk\n";

const BANNER: &str = "\u{21c4} disk changed  ^M merge";

/// The footer row as actually rendered — the one row carrying the
/// `Ln n, Col n` position readout, whose right side hosts the persistent
/// `⇄ ` diverged marker.
fn footer_row(app: &App) -> String {
    testgrid::grid(app, 100, 24)
        .into_iter()
        .rev()
        .find(|row| row.contains(", Col "))
        .unwrap_or_default()
}

fn diverged_affordances_shown(app: &App) -> bool {
    footer::footer_text(app).contains(BANNER) && footer_row(app).contains("\u{21c4} Ln")
}

/// Opens `/doc.md` seeded with `ANCESTOR`, types one edit on each of the
/// two conflict lines, rewrites the disk to `THEIRS` behind the editor's
/// back, and reprobes — the standard two-conflict `Diverged` starting
/// state.
fn open_two_conflict_diverged() -> (Session, DocumentId, DocumentId) {
    let mut session = Session::open("/doc.md", ANCESTOR);
    let doc_id = session.app().active;
    let draft_id = untitled_draft(session.app(), doc_id);

    assert!(session.key(ch('X')).is_none());
    for _ in 0..4 {
        assert!(session.key(bare(KeyCode::Down)).is_none());
    }
    assert!(session.key(bare(KeyCode::End)).is_none());
    assert!(session.key(ch('Z')).is_none());
    assert!(session.deliver_db_all().is_none());

    external_write(session.app().vfs.as_ref(), THEIRS);
    reprobe(&mut session, draft_id, doc_id);
    assert_eq!(
        session.app().doc(doc_id).unwrap().last_sync,
        Some(SyncKind::Diverged)
    );
    (session, doc_id, draft_id)
}

/// `^M`, resolves both conflicts (keep-yours then take-disk), and delivers
/// every ack the entry install and the resolutions enqueued — a fully
/// completed merge.
fn complete_by_resolving_all(session: &mut Session) {
    assert!(session.key(ctrl('m')).is_none());
    assert!(session.deliver_db().is_none());
    assert!(matches!(session.app().merge, MergeState::Active { .. }));
    assert!(session.key(take_ours()).is_none());
    assert!(session.key(take_theirs()).is_none());
    assert_eq!(session.app().merge, MergeState::Inactive);
    assert!(session.deliver_db_all().is_none());
}

/// An external disk change plus a reprobe classifies `Diverged` and raises
/// every divergence affordance at once: the footer's `⇄ disk changed —
/// [^M]erge` banner and the persistent `⇄ ` marker beside the position
/// readout.
#[test]
fn external_change_and_reprobe_raise_the_diverged_banner_and_marker() {
    let (session, _doc_id, _draft) = open_two_conflict_diverged();

    assert!(
        footer::footer_text(session.app()).contains(BANNER),
        "expected the disk-changed banner, got {:?}",
        footer::footer_text(session.app())
    );
    assert!(
        footer_row(session.app()).contains("\u{21c4} Ln"),
        "expected the persistent diverged marker beside the position readout, got {:?}",
        footer_row(session.app())
    );
}

/// A `DiskAhead` clean-buffer merge completes on the fast path: merge ends
/// `Inactive` with `last_sync == Clean`, every divergence affordance is
/// gone, and a reprobe against the untouched disk STAYS `Clean` — never
/// `Diverged` again.
#[test]
fn clean_merge_completion_retires_affordances_and_reprobe_stays_clean() {
    let mut session = Session::open("/doc.md", "hello");
    let doc_id = session.app().active;
    let draft_id = untitled_draft(session.app(), doc_id);

    external_write(session.app().vfs.as_ref(), b"hello world");
    reprobe(&mut session, draft_id, doc_id);
    assert_eq!(
        session.app().doc(doc_id).unwrap().last_sync,
        Some(SyncKind::DiskAhead)
    );

    assert!(session.key(ctrl('m')).is_none());
    assert!(session.deliver_db_all().is_none());

    assert_eq!(session.app().merge, MergeState::Inactive);
    assert_eq!(
        session.app().doc(doc_id).unwrap().last_sync,
        Some(SyncKind::Clean)
    );
    assert!(
        !diverged_affordances_shown(session.app()),
        "completion must retire the banner and marker, got {:?} / {:?}",
        footer::footer_text(session.app()),
        footer_row(session.app())
    );
    assert!(
        !footer::footer_text(session.app()).contains("merge"),
        "the ^M hint must not be offered with nothing to merge: {:?}",
        footer::footer_text(session.app())
    );

    reprobe(&mut session, draft_id, doc_id);
    assert_eq!(
        session.app().doc(doc_id).unwrap().last_sync,
        Some(SyncKind::Clean),
        "a reprobe against an untouched disk must never re-diverge"
    );
}

/// Resolving every conflict completes the merge: `Inactive`, `last_sync ==
/// BufferAhead`, affordances gone — and the loop-killer: a reprobe with the
/// disk untouched stays `BufferAhead`, NOT `Diverged`.
#[test]
fn resolve_all_completion_retires_affordances_and_reprobe_stays_buffer_ahead() {
    let (mut session, doc_id, draft_id) = open_two_conflict_diverged();

    complete_by_resolving_all(&mut session);
    assert_eq!(
        session.app().doc(doc_id).unwrap().last_sync,
        Some(SyncKind::BufferAhead)
    );
    assert!(
        !diverged_affordances_shown(session.app()),
        "completion must retire the banner and marker, got {:?} / {:?}",
        footer::footer_text(session.app()),
        footer_row(session.app())
    );

    reprobe(&mut session, draft_id, doc_id);
    assert_eq!(
        session.app().doc(doc_id).unwrap().last_sync,
        Some(SyncKind::BufferAhead),
        "a reprobe against an untouched disk must never re-diverge — this is the loop"
    );
    assert!(
        !diverged_affordances_shown(session.app()),
        "the reprobe must not resurrect the banner/marker, got {:?} / {:?}",
        footer::footer_text(session.app()),
        footer_row(session.app())
    );
}

/// A flag-only completion pushes NO journal step after the install, so
/// the resolve observation sits at exactly the journal head — the precise
/// shape that used to make `ancestor_at` skip it and fabricate `Diverged`
/// on the very next probe. It must classify `BufferAhead`.
#[test]
fn flag_only_completion_reprobe_is_not_diverged() {
    let (mut session, doc_id, draft_id) = open_two_conflict_diverged();

    assert!(session.key(ctrl('m')).is_none());
    assert!(session.deliver_db().is_none());
    assert!(matches!(session.app().merge, MergeState::Active { .. }));
    assert!(session.key(take_ours()).is_none());
    assert!(session.key(take_ours()).is_none());
    assert_eq!(session.app().merge, MergeState::Inactive);
    assert_eq!(
        session.app().doc(doc_id).unwrap().last_sync,
        Some(SyncKind::BufferAhead)
    );
    assert!(session.deliver_db_all().is_none());

    reprobe(&mut session, draft_id, doc_id);
    assert_eq!(
        session.app().doc(doc_id).unwrap().last_sync,
        Some(SyncKind::BufferAhead),
        "a resolve observation at the journal head must be its own ancestor, not Diverged"
    );
}

/// The invited "merge complete — ⌘S to save" actually works: the
/// materialize commits with no CAS refusal, no Guard, and the document
/// ends clean with the merged bytes on disk.
#[test]
fn save_after_completion_commits_without_cas_refusal() {
    let (mut session, doc_id, _draft) = open_two_conflict_diverged();

    complete_by_resolving_all(&mut session);
    let merged = session
        .app()
        .doc(doc_id)
        .unwrap()
        .buffer
        .content()
        .to_string();

    save_and_ack(&mut session);

    assert!(
        session.app().guard.is_none(),
        "the post-merge save must not raise a Guard, got {:?}",
        session.app().guard.as_ref().map(|p| p.kind.clone())
    );
    assert!(
        !rune_tui::messages::log_text(session.app()).contains("save refused"),
        "the post-merge save must not CAS-refuse, log: {:?}",
        rune_tui::messages::log_text(session.app())
    );
    assert!(
        !session.app().doc(doc_id).unwrap().is_dirty(),
        "the document must be clean after the invited save"
    );
    assert_eq!(
        session.app().vfs.read(Path::new("/doc.md")).unwrap(),
        merged.as_bytes(),
        "the merged bytes must be what reached the disk"
    );
}

/// A SECOND external write landing between merge completion and the invited
/// ⌘S still hash-mismatches: the save CAS-refuses into the disk-conflict
/// Guard and the interloper's bytes stay untouched — the merge's CAS
/// advance moved the baseline forward, it did not blunt it.
#[test]
fn second_external_write_after_completion_still_cas_refuses_into_the_guard() {
    let (mut session, doc_id, _draft) = open_two_conflict_diverged();

    complete_by_resolving_all(&mut session);
    external_write(session.app().vfs.as_ref(), b"interloper wrote this\n");

    assert!(session.key(sup('s')).is_none());
    drain_materialize_round_trip_unchecked(&mut session, doc_id);

    let Some(prompt) = &session.app().guard else {
        panic!("expected the disk-conflict Guard after the second external write");
    };
    assert_eq!(prompt.doc, doc_id);
    assert!(matches!(prompt.kind, GuardKind::DiskConflict));
    assert_eq!(
        session.app().vfs.read(Path::new("/doc.md")).unwrap(),
        b"interloper wrote this\n",
        "a refused save must leave the interloper's bytes untouched"
    );
}

/// Esc-out with unresolved blocks is not a dead end: the entry-time
/// resolve observation is abandoned, so a reprobe classifies the
/// marker-filled buffer `Diverged` again, the banner and marker return,
/// and `^M` re-enters a REAL merge (`Active`, not a "nothing to merge"
/// refusal).
#[test]
fn escape_out_with_unresolved_blocks_restores_affordances_and_ctrl_m_retries() {
    let (mut session, doc_id, draft_id) = open_two_conflict_diverged();

    assert!(session.key(ctrl('m')).is_none());
    assert!(session.deliver_db().is_none());
    assert!(matches!(session.app().merge, MergeState::Active { .. }));
    assert!(session.key(take_ours()).is_none());
    assert!(session.key(bare(KeyCode::Escape)).is_none());
    assert_eq!(session.app().merge, MergeState::Inactive);
    assert_eq!(
        session.app().doc(doc_id).unwrap().buffer.content(),
        "Xone\ntwo\nthree\nfour\nfiveZ\n",
        "Esc-out keeps the working form's bytes in place"
    );
    assert!(session.deliver_db_all().is_none());

    reprobe(&mut session, draft_id, doc_id);
    assert_eq!(
        session.app().doc(doc_id).unwrap().last_sync,
        Some(SyncKind::Diverged),
        "after the abandon, a reprobe must classify the marker buffer Diverged again"
    );
    assert!(
        diverged_affordances_shown(session.app()),
        "Esc-out must bring the banner and marker back, got {:?} / {:?}",
        footer::footer_text(session.app()),
        footer_row(session.app())
    );

    assert!(session.key(ctrl('m')).is_none());
    assert!(
        matches!(session.app().merge, MergeState::Pending { doc, .. } if doc == doc_id),
        "^M after Esc-out must start a fresh merge attempt, got {:?}",
        session.app().merge
    );
    assert!(session.deliver_db().is_none());
    assert!(
        matches!(session.app().merge, MergeState::Active { .. }),
        "the retry must land in a real resolver, not a refusal — got {:?}, log {:?}",
        session.app().merge,
        rune_tui::messages::log_text(session.app())
    );
}

/// Esc-out then ⌘S must be refused into the disk-conflict Guard: the buffer
/// still holds the conflict-marker working form and still does not descend
/// from what is on disk, so a single keystroke can never publish it over
/// the external bytes.
#[test]
fn escape_out_then_save_is_refused_into_the_guard() {
    let (mut session, doc_id, _draft) = open_two_conflict_diverged();

    assert!(session.key(ctrl('m')).is_none());
    assert!(session.deliver_db().is_none());
    assert!(matches!(session.app().merge, MergeState::Active { .. }));
    assert!(session.key(bare(KeyCode::Escape)).is_none());
    assert_eq!(session.app().merge, MergeState::Inactive);
    assert!(session.deliver_db_all().is_none());

    save_expecting_refusal(&mut session);

    let Some(prompt) = &session.app().guard else {
        panic!("expected the disk-conflict Guard, not a silent marker publish");
    };
    assert_eq!(prompt.doc, doc_id);
    assert!(matches!(prompt.kind, GuardKind::DiskConflict));
    assert_eq!(
        session.app().vfs.read(Path::new("/doc.md")).unwrap(),
        THEIRS,
        "the refused save must leave the external bytes untouched — never conflict markers"
    );
}
