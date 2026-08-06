//! Post-merge sync regression suite (plan "merge-ux" Step 7): the
//! infinite re-prompt loop is dead. External divergence raises the ⇄
//! affordances, completing a merge retires them, a reprobe with an
//! untouched disk must NOT re-classify `Diverged` (the loop-killer), the
//! invited ⌘S commits without a CAS refusal, and a genuinely fresh second
//! external write still CAS-refuses into the disk-conflict Guard. Pulls
//! shared fixtures from `merge_common`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod merge_common;

use std::path::Path;
use std::sync::Arc;

use rune_db::SyncKind;
use rune_tui::app::App;
use rune_tui::db::DbBridge;
use rune_tui::document::DocumentId;
use rune_tui::footer;
use rune_tui::guard::GuardKind;
use rune_tui::keymap::KeyCode;
use rune_tui::merge::MergeState;
use rune_tui::testgrid;
use rune_vfs::{Mem, Vfs};

use merge_common::{
    app_with_store, bare, ch, ctrl, drain_all_ops_for, drain_one_op_for, external_write, press_key,
    publish, reprobe, save_and_ack,
};

/// Both sides edit line 1 AND line 5 differently, with three untouched
/// context lines between — two separate conflicts under any diff engine
/// (the same spacing `merge_resolver.rs`'s fixture uses).
const ANCESTOR: &[u8] = b"one\ntwo\nthree\nfour\nfive\n";
const THEIRS: &[u8] = b"one disk\ntwo\nthree\nfour\nfive disk\n";

const BANNER: &str = "\u{21c4} disk changed \u{2014} [^M]erge";

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
fn open_two_conflict_diverged(label: &str) -> (App, Arc<DbBridge>, DocumentId, DocumentId) {
    let mem = Mem::new();
    publish(&mem, Path::new("/doc.md"), ANCESTOR);
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(mem);

    let (mut app, bridge) = app_with_store(label, Arc::clone(&vfs));
    let draft_id = app.active;
    rune_tui::workspace::open_path(&mut app, Path::new("/doc.md"));
    let doc_id = app.active;
    drain_one_op_for(&mut app, &bridge, doc_id);

    press_key(&mut app, ch('X'));
    for _ in 0..4 {
        press_key(&mut app, bare(KeyCode::Down));
    }
    press_key(&mut app, bare(KeyCode::End));
    press_key(&mut app, ch('Z'));
    drain_all_ops_for(&mut app, &bridge, doc_id);

    external_write(vfs.as_ref(), THEIRS);
    reprobe(&mut app, &bridge, draft_id, doc_id);
    assert_eq!(app.doc(doc_id).unwrap().last_sync, Some(SyncKind::Diverged));
    (app, bridge, doc_id, draft_id)
}

/// `^M`, resolves both conflicts (`O` then `T`), and drains every ack the
/// entry install and the accepts enqueued — a fully completed merge.
fn complete_by_resolving_all(app: &mut App, bridge: &DbBridge, doc_id: DocumentId) {
    press_key(app, ctrl('m'));
    drain_one_op_for(app, bridge, doc_id);
    assert!(matches!(app.merge, MergeState::Active { .. }));
    press_key(app, ch('o'));
    press_key(app, ch('t'));
    assert_eq!(app.merge, MergeState::Inactive);
    drain_all_ops_for(app, bridge, doc_id);
}

/// An external disk change plus a reprobe classifies `Diverged` and raises
/// every divergence affordance at once: the footer's `⇄ disk changed —
/// [^M]erge` banner and the persistent `⇄ ` marker beside the position
/// readout.
#[test]
fn external_change_and_reprobe_raise_the_diverged_banner_and_marker() {
    let (app, _bridge, _doc_id, _draft) = open_two_conflict_diverged("post-sync-affordances");

    assert!(
        footer::footer_text(&app).contains(BANNER),
        "expected the disk-changed banner, got {:?}",
        footer::footer_text(&app)
    );
    assert!(
        footer_row(&app).contains("\u{21c4} Ln"),
        "expected the persistent diverged marker beside the position readout, got {:?}",
        footer_row(&app)
    );
}

/// A `DiskAhead` clean-buffer merge completes on the fast path: merge ends
/// `Inactive` with `last_sync == Clean`, every divergence affordance is
/// gone, and a reprobe against the untouched disk STAYS `Clean` — never
/// `Diverged` again.
#[test]
fn clean_merge_completion_retires_affordances_and_reprobe_stays_clean() {
    let mem = Mem::new();
    publish(&mem, Path::new("/doc.md"), b"hello");
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(mem);

    let (mut app, bridge) = app_with_store("post-sync-clean-merge", Arc::clone(&vfs));
    let draft_id = app.active;
    rune_tui::workspace::open_path(&mut app, Path::new("/doc.md"));
    let doc_id = app.active;
    drain_one_op_for(&mut app, &bridge, doc_id);

    external_write(vfs.as_ref(), b"hello world");
    reprobe(&mut app, &bridge, draft_id, doc_id);
    assert_eq!(
        app.doc(doc_id).unwrap().last_sync,
        Some(SyncKind::DiskAhead)
    );

    press_key(&mut app, ctrl('m'));
    drain_all_ops_for(&mut app, &bridge, doc_id);

    assert_eq!(app.merge, MergeState::Inactive);
    assert_eq!(app.doc(doc_id).unwrap().last_sync, Some(SyncKind::Clean));
    assert!(
        !diverged_affordances_shown(&app),
        "completion must retire the banner and marker, got {:?} / {:?}",
        footer::footer_text(&app),
        footer_row(&app)
    );
    assert!(
        !footer::footer_text(&app).contains("merge"),
        "the ^M hint must not be offered with nothing to merge: {:?}",
        footer::footer_text(&app)
    );

    reprobe(&mut app, &bridge, draft_id, doc_id);
    assert_eq!(
        app.doc(doc_id).unwrap().last_sync,
        Some(SyncKind::Clean),
        "a reprobe against an untouched disk must never re-diverge"
    );
}

/// Resolving every conflict completes the merge: `Inactive`, `last_sync ==
/// BufferAhead`, affordances gone — and the loop-killer: a reprobe with the
/// disk untouched stays `BufferAhead`, NOT `Diverged`.
#[test]
fn resolve_all_completion_retires_affordances_and_reprobe_stays_buffer_ahead() {
    let (mut app, bridge, doc_id, draft_id) = open_two_conflict_diverged("post-sync-resolve-all");

    complete_by_resolving_all(&mut app, &bridge, doc_id);
    assert_eq!(
        app.doc(doc_id).unwrap().last_sync,
        Some(SyncKind::BufferAhead)
    );
    assert!(
        !diverged_affordances_shown(&app),
        "completion must retire the banner and marker, got {:?} / {:?}",
        footer::footer_text(&app),
        footer_row(&app)
    );

    reprobe(&mut app, &bridge, draft_id, doc_id);
    assert_eq!(
        app.doc(doc_id).unwrap().last_sync,
        Some(SyncKind::BufferAhead),
        "a reprobe against an untouched disk must never re-diverge — this is the loop"
    );
    assert!(
        !diverged_affordances_shown(&app),
        "the reprobe must not resurrect the banner/marker, got {:?} / {:?}",
        footer::footer_text(&app),
        footer_row(&app)
    );
}

/// The all-`[B]`oth completion pushes NO journal step after the install, so
/// the resolve observation sits at exactly the journal head — the precise
/// shape that used to make `ancestor_at` skip it and fabricate `Diverged`
/// on the very next probe. It must classify `BufferAhead`.
#[test]
fn all_both_completion_reprobe_is_not_diverged() {
    let (mut app, bridge, doc_id, draft_id) = open_two_conflict_diverged("post-sync-all-both");

    press_key(&mut app, ctrl('m'));
    drain_one_op_for(&mut app, &bridge, doc_id);
    assert!(matches!(app.merge, MergeState::Active { .. }));
    press_key(&mut app, ch('b'));
    press_key(&mut app, ch('b'));
    assert_eq!(app.merge, MergeState::Inactive);
    assert_eq!(
        app.doc(doc_id).unwrap().last_sync,
        Some(SyncKind::BufferAhead)
    );
    drain_all_ops_for(&mut app, &bridge, doc_id);

    reprobe(&mut app, &bridge, draft_id, doc_id);
    assert_eq!(
        app.doc(doc_id).unwrap().last_sync,
        Some(SyncKind::BufferAhead),
        "a resolve observation at the journal head must be its own ancestor, not Diverged"
    );
}

/// The invited "merge complete — ⌘S to save" actually works: the
/// materialize commits with no CAS refusal, no Guard, and the document
/// ends clean with the merged bytes on disk.
#[test]
fn save_after_completion_commits_without_cas_refusal() {
    let (mut app, bridge, doc_id, _draft) = open_two_conflict_diverged("post-sync-save-ok");

    complete_by_resolving_all(&mut app, &bridge, doc_id);
    let merged = app.doc(doc_id).unwrap().buffer.content().to_string();

    save_and_ack(&mut app, &bridge, doc_id);

    assert!(
        app.guard.is_none(),
        "the post-merge save must not raise a Guard, got {:?}",
        app.guard.as_ref().map(|p| p.kind.clone())
    );
    assert!(
        !rune_tui::messages::log_text(&app).contains("save refused"),
        "the post-merge save must not CAS-refuse, log: {:?}",
        rune_tui::messages::log_text(&app)
    );
    app.recompute_dirty(doc_id);
    assert!(
        !app.doc(doc_id).unwrap().is_dirty(),
        "the document must be clean after the invited save"
    );
    assert_eq!(
        app.vfs.read(Path::new("/doc.md")).unwrap(),
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
    let (mut app, bridge, doc_id, _draft) = open_two_conflict_diverged("post-sync-second-write");

    complete_by_resolving_all(&mut app, &bridge, doc_id);
    external_write(app.vfs.as_ref(), b"interloper wrote this\n");

    save_and_ack(&mut app, &bridge, doc_id);

    let Some(prompt) = &app.guard else {
        panic!("expected the disk-conflict Guard after the second external write");
    };
    assert_eq!(prompt.doc, doc_id);
    assert!(matches!(prompt.kind, GuardKind::DiskConflict { .. }));
    assert_eq!(
        app.vfs.read(Path::new("/doc.md")).unwrap(),
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
    let (mut app, bridge, doc_id, draft_id) = open_two_conflict_diverged("post-sync-esc-out");

    press_key(&mut app, ctrl('m'));
    drain_one_op_for(&mut app, &bridge, doc_id);
    assert!(matches!(app.merge, MergeState::Active { .. }));
    press_key(&mut app, ch('o'));
    press_key(&mut app, bare(KeyCode::Escape));
    assert_eq!(app.merge, MergeState::Inactive);
    assert!(
        app.doc(doc_id)
            .unwrap()
            .buffer
            .content()
            .contains("<<<<<<< editor\n"),
        "the unresolved block's markers must remain after Esc-out"
    );
    drain_all_ops_for(&mut app, &bridge, doc_id);

    reprobe(&mut app, &bridge, draft_id, doc_id);
    assert_eq!(
        app.doc(doc_id).unwrap().last_sync,
        Some(SyncKind::Diverged),
        "after the abandon, a reprobe must classify the marker buffer Diverged again"
    );
    assert!(
        diverged_affordances_shown(&app),
        "Esc-out must bring the banner and marker back, got {:?} / {:?}",
        footer::footer_text(&app),
        footer_row(&app)
    );

    press_key(&mut app, ctrl('m'));
    assert!(
        matches!(app.merge, MergeState::Pending { doc, .. } if doc == doc_id),
        "^M after Esc-out must start a fresh merge attempt, got {:?}",
        app.merge
    );
    drain_one_op_for(&mut app, &bridge, doc_id);
    assert!(
        matches!(app.merge, MergeState::Active { .. }),
        "the retry must land in a real resolver, not a refusal — got {:?}, log {:?}",
        app.merge,
        rune_tui::messages::log_text(&app)
    );
}

/// Esc-out then ⌘S must CAS-refuse into the disk-conflict Guard: the save
/// baseline never advanced at resolver entry, so a single keystroke can
/// never silently publish the conflict-marker working form over the
/// external bytes on disk.
#[test]
fn escape_out_then_save_cas_refuses_into_the_guard() {
    let (mut app, bridge, doc_id, _draft) = open_two_conflict_diverged("post-sync-esc-save");

    press_key(&mut app, ctrl('m'));
    drain_one_op_for(&mut app, &bridge, doc_id);
    assert!(matches!(app.merge, MergeState::Active { .. }));
    press_key(&mut app, bare(KeyCode::Escape));
    assert_eq!(app.merge, MergeState::Inactive);
    drain_all_ops_for(&mut app, &bridge, doc_id);

    save_and_ack(&mut app, &bridge, doc_id);

    let Some(prompt) = &app.guard else {
        panic!("expected the disk-conflict Guard, not a silent marker publish");
    };
    assert_eq!(prompt.doc, doc_id);
    assert!(matches!(prompt.kind, GuardKind::DiskConflict { .. }));
    assert_eq!(
        app.vfs.read(Path::new("/doc.md")).unwrap(),
        THEIRS,
        "the refused save must leave the external bytes untouched — never conflict markers"
    );
}
