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
use rune_tui::keymap::{KeyCode, Mods};
use rune_tui::merge::{MergeState, Resolution};

use merge_common::{
    bare, ch, chord, ctrl, external_write, next_hunk, prev_hunk, reprobe, sup, take_ours,
    take_theirs, untitled_draft,
};

const ANCESTOR: &str = "one\ntwo\nthree\nfour\nfive\n";
const THEIRS: &[u8] = b"one disk\ntwo\nthree\nfour\nfive disk\n";
const OURS: &str = "Xone\ntwo\nthree\nfour\nfiveZ\n";

fn enter_two_conflict_merge() -> (Session, DocumentId) {
    let mut session = Session::open("/doc.md", ANCESTOR);
    let doc_id = session.app().active;
    let draft_id = untitled_draft(session.app(), doc_id);

    assert!(session.key(ch('X')).is_none());
    for _ in 0..4 {
        assert!(session.key(bare(KeyCode::Down)).is_none());
    }
    assert!(session.key(bare(KeyCode::End)).is_none());
    assert!(session.key(ch('Z')).is_none());
    assert_eq!(session.app().doc(doc_id).unwrap().buffer.content(), OURS);
    assert!(session.deliver_db_all().is_none());

    external_write(session.app().vfs.as_ref(), THEIRS);
    reprobe(&mut session, draft_id, doc_id);
    assert_eq!(
        session.app().doc(doc_id).unwrap().last_sync,
        Some(SyncKind::Diverged)
    );

    assert!(session.key(ctrl('m')).is_none());
    assert!(session.deliver_db().is_none());

    let MergeState::Active { session: merge, .. } = &session.app().merge else {
        panic!("expected an active resolver, got {:?}", session.app().merge);
    };
    assert_eq!(
        merge.conflicts.len(),
        2,
        "fixture must produce exactly two conflicts"
    );
    assert_eq!(merge.cur, 0);
    (session, doc_id)
}

fn current_block(app: &App) -> usize {
    let MergeState::Active { session: merge, .. } = &app.merge else {
        panic!("resolver not active");
    };
    merge.cur
}

fn resolution_of(app: &App, idx: usize) -> Resolution {
    let MergeState::Active { session: merge, .. } = &app.merge else {
        panic!("resolver not active");
    };
    merge.conflicts[idx].block.resolution
}

#[test]
fn merge_entry_shows_ours_in_place_and_theirs_in_the_left_pane() {
    let (session, doc_id) = enter_two_conflict_merge();

    let doc = session.app().doc(doc_id).unwrap();
    assert_eq!(doc.buffer.content(), OURS);
    assert!(!doc.buffer.content().contains("<<<<<<<"));
    let diff = session.app().diff.as_ref().expect("pane view installed");
    assert_eq!(diff.right, doc_id);
    assert_eq!(diff.left.buffer.content(), String::from_utf8_lossy(THEIRS));
}

#[test]
fn hunk_navigation_cycles_and_wraps_in_both_directions() {
    let (mut session, _doc) = enter_two_conflict_merge();

    assert!(session.key(next_hunk()).is_none());
    assert_eq!(current_block(session.app()), 1);
    assert!(session.key(next_hunk()).is_none());
    assert_eq!(current_block(session.app()), 0, "next wraps around");
    assert!(session.key(prev_hunk()).is_none());
    assert_eq!(current_block(session.app()), 1, "prev wraps around");
    assert!(session.key(prev_hunk()).is_none());
    assert_eq!(current_block(session.app()), 0);
    assert!(
        rune_tui::messages::newest_text(session.app())
            .unwrap_or_default()
            .contains("conflict 1/2"),
        "navigation must report the position, got {:?}",
        rune_tui::messages::newest_text(session.app())
    );
}

#[test]
fn navigation_still_reaches_a_resolved_conflict_so_it_can_be_reopened() {
    let (mut session, _doc) = enter_two_conflict_merge();

    assert!(session.key(take_ours()).is_none());
    assert_eq!(resolution_of(session.app(), 0), Resolution::KeptOurs);
    assert_eq!(
        current_block(session.app()),
        1,
        "resolving advances to the next unresolved conflict"
    );

    assert!(session.key(next_hunk()).is_none());
    assert_eq!(
        current_block(session.app()),
        0,
        "the resolved conflict stays reachable"
    );
    assert!(session.key(take_ours()).is_none());
    assert_eq!(
        resolution_of(session.app(), 0),
        Resolution::Unresolved,
        "a second keep-yours on an unedited conflict reopens it"
    );
}

#[test]
fn keep_ours_is_flag_only_and_take_theirs_is_one_journal_step() {
    let (mut session, doc_id) = enter_two_conflict_merge();

    let pos_before = session.app().doc(doc_id).unwrap().journal.pos();
    assert!(session.key(take_ours()).is_none());
    let doc = session.app().doc(doc_id).unwrap();
    assert_eq!(
        doc.journal.pos(),
        pos_before,
        "keep-yours on an unedited conflict is a flag, not an edit"
    );
    assert_eq!(doc.buffer.content(), OURS);

    let pos_before = session.app().doc(doc_id).unwrap().journal.pos();
    assert!(session.key(take_theirs()).is_none());
    let doc = session.app().doc(doc_id).unwrap();
    assert_eq!(
        doc.journal.pos(),
        pos_before + 1,
        "take-theirs is one journal step"
    );
    assert_eq!(
        doc.buffer.content(),
        "Xone\ntwo\nthree\nfour\nfive disk\n",
        "keep-yours kept line 1, take-theirs adopted line 5"
    );

    assert_eq!(session.app().merge, MergeState::Inactive);
    assert!(
        session.app().diff.is_none(),
        "completion tears the pane view down"
    );
    assert!(
        !session
            .app()
            .doc(doc_id)
            .unwrap()
            .file_name()
            .ends_with(": editor <-> disk"),
        "title must revert on exit"
    );
    assert!(
        rune_tui::messages::newest_text(session.app())
            .unwrap_or_default()
            .contains("merge complete"),
        "expected the merge-complete status, got {:?}",
        rune_tui::messages::newest_text(session.app())
    );
}

#[test]
fn fold_mode_at_the_default_width_keeps_the_save_gate_and_take_theirs_working() {
    let (mut session, doc_id) = enter_two_conflict_merge();

    let geo = rune_tui::layout::geometry(
        ratatui::layout::Rect::new(0, 0, session.app().frame_width, session.app().frame_height),
        session.app(),
    );
    assert!(
        geo.diff_left.is_none(),
        "the session's default width must be narrow enough to fold"
    );

    assert!(session.key(take_theirs()).is_none());
    assert_eq!(resolution_of(session.app(), 0), Resolution::TookTheirs);

    assert!(session.key(sup('s')).is_none());
    assert!(
        rune_tui::messages::newest_text(session.app())
            .unwrap_or_default()
            .contains("conflict(s) to resolve"),
        "one conflict is still unresolved, folded or not"
    );

    assert!(session.key(take_ours()).is_none());
    assert_eq!(session.app().merge, MergeState::Inactive);

    assert!(session.key(sup('s')).is_none());
    assert!(
        !rune_tui::messages::newest_text(session.app())
            .unwrap_or_default()
            .contains("conflict(s) to resolve"),
        "after full resolution ⌘S must not be refused while folded, got {:?}",
        rune_tui::messages::newest_text(session.app())
    );
    assert_eq!(
        session.app().doc(doc_id).unwrap().buffer.content(),
        "one disk\ntwo\nthree\nfour\nfiveZ\n"
    );
}

#[test]
fn save_is_refused_while_unresolved_and_allowed_after() {
    let (mut session, doc_id) = enter_two_conflict_merge();

    let before = session
        .app()
        .doc(doc_id)
        .unwrap()
        .buffer
        .content()
        .to_string();
    assert!(session.key(sup('s')).is_none());
    assert!(
        rune_tui::messages::newest_text(session.app())
            .unwrap_or_default()
            .contains("conflict(s) to resolve"),
        "expected the save refusal, got {:?}",
        rune_tui::messages::newest_text(session.app())
    );
    assert_eq!(
        session.app().doc(doc_id).unwrap().buffer.content(),
        before,
        "a refused save must not touch the buffer"
    );

    assert!(session.key(take_ours()).is_none());
    assert!(session.key(take_theirs()).is_none());
    assert_eq!(session.app().merge, MergeState::Inactive);
    assert!(session.key(sup('s')).is_none());
    assert!(
        !rune_tui::messages::newest_text(session.app())
            .unwrap_or_default()
            .contains("conflict(s) to resolve"),
        "after full resolution ⌘S must not be refused, got {:?}",
        rune_tui::messages::newest_text(session.app())
    );
}

#[test]
fn typing_inside_a_conflict_edits_the_buffer_and_marks_it_hand_edited() {
    let (mut session, doc_id) = enter_two_conflict_merge();

    let pos_before = session.app().doc(doc_id).unwrap().journal.pos();
    assert!(session.key(bare(KeyCode::Right)).is_none());
    assert!(session.key(ch('q')).is_none());

    let doc = session.app().doc(doc_id).unwrap();
    assert_eq!(
        doc.buffer.content(),
        "Xqone\ntwo\nthree\nfour\nfiveZ\n",
        "printable keys fall through to ordinary editing in merge mode"
    );
    assert_eq!(doc.journal.pos(), pos_before + 1);
    assert_eq!(
        resolution_of(session.app(), 0),
        Resolution::HandEdited,
        "an edit inside a conflict's range marks it hand-edited"
    );
    assert_eq!(
        resolution_of(session.app(), 1),
        Resolution::Unresolved,
        "the untouched conflict stays unresolved"
    );

    assert!(session.key(sup('s')).is_none());
    assert!(
        rune_tui::messages::newest_text(session.app())
            .unwrap_or_default()
            .contains("conflict(s) to resolve"),
        "the remaining unresolved conflict still gates the save"
    );
}

#[test]
fn escape_exits_in_place_keeping_the_buffer() {
    let (mut session, doc_id) = enter_two_conflict_merge();

    assert!(session.key(take_ours()).is_none());
    let before = session
        .app()
        .doc(doc_id)
        .unwrap()
        .buffer
        .content()
        .to_string();
    assert!(session.key(bare(KeyCode::Escape)).is_none());

    assert_eq!(session.app().merge, MergeState::Inactive);
    assert!(
        session.app().diff.is_none(),
        "exit tears the pane view down"
    );
    let doc = session.app().doc(doc_id).unwrap();
    assert_eq!(doc.buffer.content(), before, "exit in place edits nothing");
    assert!(!doc.file_name().ends_with(": editor <-> disk"));
    assert!(
        rune_tui::messages::newest_text(session.app())
            .unwrap_or_default()
            .contains("1 unresolved conflict"),
        "expected the exit summary, got {:?}",
        rune_tui::messages::newest_text(session.app())
    );
}

#[test]
fn dirty_close_guard_save_during_unresolved_merge_refuses_and_writes_nothing() {
    let (mut session, doc_id) = enter_two_conflict_merge();
    let disk_before = session.app().vfs.read(Path::new("/doc.md")).unwrap();

    assert!(session.key(ctrl('w')).is_none());
    assert!(
        matches!(
            &session.app().guard,
            Some(prompt) if prompt.kind == rune_tui::guard::GuardKind::DirtyClose
        ),
        "closing the dirty working form must arm the DirtyClose guard"
    );

    assert!(session.key(ch('s')).is_none());

    assert!(
        rune_tui::messages::newest_text(session.app())
            .unwrap_or_default()
            .contains("conflict(s) to resolve"),
        "expected the merge save refusal, got {:?}",
        rune_tui::messages::newest_text(session.app())
    );
    assert!(
        session.app().doc(doc_id).is_some(),
        "the document must stay open"
    );
    assert!(
        !session.app().doc(doc_id).unwrap().save_in_flight(),
        "no save may start while the resolver has unresolved conflicts"
    );
    assert!(matches!(
        session.app().merge,
        MergeState::Active { doc, .. } if doc == doc_id
    ));
    assert_eq!(
        session.app().vfs.read(Path::new("/doc.md")).unwrap(),
        disk_before,
        "the half-merged working form must never reach the disk file"
    );
}

#[test]
fn help_lists_the_diff_view_bindings() {
    let help = rune_tui::help::help_markdown();
    assert!(
        help.contains("## Diff View"),
        "missing diff view section: {help}"
    );
    for label in ["prev hunk", "next hunk", "take theirs", "take ours"] {
        assert!(help.contains(label), "missing help row {label:?}");
    }
    assert!(
        !help.contains("## Merge"),
        "the marker-era merge section must be gone: {help}"
    );
}

#[test]
fn merge_active_arrow_chords_never_silently_scroll_or_swallow_input() {
    let (mut session, doc_id) = enter_two_conflict_merge();

    // The first unresolved conflict lands the caret on line 0, where
    // `AddCursorAbove` is a legitimate no-op — move down first through
    // ordinary (unchorded) motion so the alt-sup case below has a real
    // line above it to add onto.
    assert!(session.key(bare(KeyCode::Down)).is_none());
    assert!(session.key(bare(KeyCode::Down)).is_none());

    let content_before = session
        .app()
        .doc(doc_id)
        .unwrap()
        .buffer
        .content()
        .to_string();
    let scroll_before = session.app().doc(doc_id).unwrap().viewport.scroll_row;
    let cursors_before = session.app().doc(doc_id).unwrap().cursors.len();

    let alt_sup = Mods {
        shift: false,
        alt: true,
        ctrl: false,
        sup: true,
    };
    let bare_sup = Mods {
        shift: false,
        alt: false,
        ctrl: false,
        sup: true,
    };

    assert!(session.key(chord(KeyCode::Up, alt_sup)).is_none());
    let after_alt_up = session.app().doc(doc_id).unwrap();
    assert_eq!(
        after_alt_up.buffer.content(),
        content_before,
        "\u{2325}\u{2318}\u{2191} must not edit the buffer"
    );
    assert_eq!(
        after_alt_up.viewport.scroll_row, scroll_before,
        "\u{2325}\u{2318}\u{2191} must not silently scroll the viewport"
    );
    assert!(
        after_alt_up.cursors.len() > cursors_before,
        "\u{2325}\u{2318}\u{2191} must fall through to a real, visible AddCursorAbove, not a swallowed no-op"
    );

    assert!(session.key(chord(KeyCode::Up, bare_sup)).is_none());
    let after_bare = session.app().doc(doc_id).unwrap();
    assert_eq!(
        after_bare.buffer.content(),
        content_before,
        "bare \u{2318}\u{2191} must not edit the buffer"
    );
    assert_eq!(
        after_bare.viewport.scroll_row, scroll_before,
        "bare \u{2318}\u{2191} must not silently scroll the viewport"
    );

    let MergeState::Active { session: merge, .. } = &session.app().merge else {
        panic!(
            "the resolver must still be active, got {:?}",
            session.app().merge
        );
    };
    assert_eq!(merge.cur, 0, "the current hunk must be untouched");
    assert_eq!(
        merge
            .conflicts
            .iter()
            .filter(|p| !p.block.resolution.is_resolved())
            .count(),
        2,
        "no chord may have silently resolved a hunk"
    );
}
