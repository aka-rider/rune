//! WP4 "Done when" integration tests for the merge resolver (plan
//! `merge-user-s-changes-with-idempotent-octopus.md`): `[`/`]` navigation,
//! O/T/B accepts, the ⌘S gate, key swallowing with feedback, and the Help
//! table. Driven through `rune_fuzz::Session`, sharing its fixtures with
//! `merge_entry.rs` via `merge_common`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod merge_common;

use std::path::Path;

use ratatui::layout::Rect;
use rune_core::coords::DisplayRow;
use rune_db::SyncKind;
use rune_fuzz::Session;
use rune_tui::app::App;
use rune_tui::document::DocumentId;
use rune_tui::keymap::{KeyCode, Mods};
use rune_tui::merge::MergeState;

use merge_common::{bare, ch, chord, ctrl, external_write, reprobe, sup, untitled_draft};

/// Both sides edit line 1 AND line 5 differently, with three untouched
/// context lines between — two separate conflicts under any diff engine.
const ANCESTOR: &str = "one\ntwo\nthree\nfour\nfive\n";
const THEIRS: &[u8] = b"one disk\ntwo\nthree\nfour\nfive disk\n";

/// Builds the standard two-conflict resolver session: ancestor on disk at
/// load, ours edits lines 1 and 5 by typing, theirs rewrites both on disk,
/// `^M` enters, and the resolver is Active with exactly two blocks.
fn enter_two_conflict_merge() -> (Session, DocumentId) {
    let mut session = Session::open("/doc.md", ANCESTOR);
    let doc_id = session.app().active;
    let draft_id = untitled_draft(session.app(), doc_id);

    // Ours: "Xone\n...\nfiveZ\n" — one edit on each conflicting line.
    assert!(session.key(ch('X')).is_none());
    for _ in 0..4 {
        assert!(session.key(bare(KeyCode::Down)).is_none());
    }
    assert!(session.key(bare(KeyCode::End)).is_none());
    assert!(session.key(ch('Z')).is_none());
    assert_eq!(
        session.app().doc(doc_id).unwrap().buffer.content(),
        "Xone\ntwo\nthree\nfour\nfiveZ\n"
    );
    assert!(session.deliver_db_all().is_none());

    external_write(session.app().vfs.as_ref(), THEIRS);
    reprobe(&mut session, draft_id, doc_id);
    assert_eq!(
        session.app().doc(doc_id).unwrap().last_sync,
        Some(SyncKind::Diverged)
    );

    assert!(session.key(ctrl('m')).is_none());
    assert!(session.deliver_db().is_none());

    let MergeState::Active { pairs, cur, .. } = &session.app().merge else {
        panic!("expected an active resolver, got {:?}", session.app().merge);
    };
    assert_eq!(pairs.len(), 2, "fixture must produce exactly two conflicts");
    assert_eq!(*cur, 0);
    (session, doc_id)
}

fn current_block(app: &App) -> usize {
    let MergeState::Active { cur, .. } = &app.merge else {
        panic!("resolver not active");
    };
    *cur
}

#[test]
fn bracket_navigation_cycles_and_skips_resolved_blocks() {
    let (mut session, _doc) = enter_two_conflict_merge();

    assert!(session.key(ch(']')).is_none());
    assert_eq!(current_block(session.app()), 1);
    assert!(session.key(ch(']')).is_none());
    assert_eq!(current_block(session.app()), 0, "next wraps around");
    assert!(session.key(ch('[')).is_none());
    assert_eq!(current_block(session.app()), 1, "prev wraps around");
    assert!(session.key(ch('[')).is_none());
    assert_eq!(current_block(session.app()), 0);

    // Resolve block 0; navigation must now skip it from either direction.
    assert!(session.key(ch('b')).is_none());
    assert_eq!(
        current_block(session.app()),
        1,
        "accept advances to next unresolved"
    );
    assert!(session.key(ch(']')).is_none());
    assert_eq!(
        current_block(session.app()),
        1,
        "the resolved block is skipped"
    );
    assert!(session.key(ch('[')).is_none());
    assert_eq!(current_block(session.app()), 1);
}

#[test]
fn ours_and_theirs_collapse_blocks_to_exact_bytes_one_journal_step_each() {
    let (mut session, doc_id) = enter_two_conflict_merge();
    let saved_name = session.app().doc(doc_id).unwrap().file_name().to_string();
    assert!(saved_name.ends_with(": editor <-> disk"));

    let pos_before = session.app().doc(doc_id).unwrap().journal.pos();
    assert!(session.key(ch('o')).is_none());
    let doc = session.app().doc(doc_id).unwrap();
    assert_eq!(doc.journal.pos(), pos_before + 1, "O is one journal step");
    assert!(doc.buffer.content().starts_with("Xone\ntwo\n"));
    assert!(
        !doc.buffer.content().starts_with("<<<<<<<"),
        "block 1 must be collapsed: {:?}",
        doc.buffer.content()
    );

    let pos_before = session.app().doc(doc_id).unwrap().journal.pos();
    assert!(session.key(ch('t')).is_none());
    let doc = session.app().doc(doc_id).unwrap();
    assert_eq!(doc.journal.pos(), pos_before + 1, "T is one journal step");
    assert_eq!(
        doc.buffer.content(),
        "Xone\ntwo\nthree\nfour\nfive disk\n",
        "O kept ours on block 1, T kept theirs on block 2"
    );

    // Decision 13: resolving the last hunk exits in place.
    assert_eq!(session.app().merge, MergeState::Inactive);
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
fn both_strips_markers_and_keeps_both_sides_as_one_edit() {
    let (mut session, doc_id) = enter_two_conflict_merge();

    let pos_before = session.app().doc(doc_id).unwrap().journal.pos();
    assert!(session.key(ch('b')).is_none());

    let doc = session.app().doc(doc_id).unwrap();
    assert_eq!(doc.journal.pos(), pos_before + 1, "B is one journal step");
    assert!(
        doc.buffer.content().starts_with("Xone\n\none disk\ntwo\n"),
        "B keeps ours then theirs with no marker lines: {:?}",
        doc.buffer.content()
    );
    let MergeState::Active { pairs, .. } = &session.app().merge else {
        panic!("resolver still active after resolving 1 of 2");
    };
    assert!(pairs[0].block.resolved);
    assert!(!pairs[1].block.resolved);
    assert_eq!(
        session
            .app()
            .doc(doc_id)
            .unwrap()
            .buffer
            .content()
            .matches("<<<<<<<")
            .count(),
        1,
        "only the unresolved block's markers remain"
    );
}

#[test]
fn all_both_resolution_leaves_zero_marker_bytes() {
    let (mut session, doc_id) = enter_two_conflict_merge();

    assert!(session.key(ch('b')).is_none());
    assert!(session.key(ch('b')).is_none());

    assert_eq!(
        session.app().merge,
        MergeState::Inactive,
        "all resolved exits merge"
    );
    let content = session
        .app()
        .doc(doc_id)
        .unwrap()
        .buffer
        .content()
        .to_string();
    for marker in ["<<<<<<<", "=======", ">>>>>>>"] {
        assert!(
            !content.contains(marker),
            "no {marker} bytes may remain: {content:?}"
        );
    }
    assert_eq!(
        content,
        "Xone\n\none disk\ntwo\nthree\nfour\nfiveZ\n\nfive disk\n"
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

    assert!(session.key(ch('o')).is_none());
    assert!(session.key(ch('t')).is_none());
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
fn unbound_keys_are_swallowed_with_feedback_while_resolving() {
    let (mut session, doc_id) = enter_two_conflict_merge();

    let before = session
        .app()
        .doc(doc_id)
        .unwrap()
        .buffer
        .content()
        .to_string();
    let pos_before = session.app().doc(doc_id).unwrap().journal.pos();
    for key in [
        ch('x'),
        ch('7'),
        bare(KeyCode::Enter),
        bare(KeyCode::Backspace),
    ] {
        assert!(session.key(key).is_none());
        assert_eq!(
            session.app().doc(doc_id).unwrap().buffer.content(),
            before,
            "{key:?} must not reach the working form"
        );
        assert!(
            rune_tui::messages::newest_text(session.app())
                .unwrap_or_default()
                .contains("merge:"),
            "{key:?} must be swallowed WITH feedback, got {:?}",
            rune_tui::messages::newest_text(session.app())
        );
    }
    assert_eq!(session.app().doc(doc_id).unwrap().journal.pos(), pos_before);
}

/// Issue #54: `⌥⌘↑`/`⌥⌘↓` (`AddCursorAbove`/`AddCursorBelow`), bare `⌘↑`, and
/// `⇧⌥↑` (`CloneLineUp`) are ordinary editor commands with no meaning while
/// the resolver owns the keyboard, and must be refused out loud rather than
/// silently re-keyed into a one-row scroll.
#[test]
fn modifier_arrow_chords_are_refused_with_feedback_while_resolving() {
    let (mut session, doc_id) = enter_two_conflict_merge();

    let before = session
        .app()
        .doc(doc_id)
        .unwrap()
        .buffer
        .content()
        .to_string();
    let pos_before = session.app().doc(doc_id).unwrap().journal.pos();
    let scroll_before = session.app().doc(doc_id).unwrap().viewport.scroll_row;
    let alt_sup = Mods {
        alt: true,
        sup: true,
        ..Mods::NONE
    };
    let sup_only = Mods {
        sup: true,
        ..Mods::NONE
    };
    let shift_alt = Mods {
        shift: true,
        alt: true,
        ..Mods::NONE
    };
    let mut posts_before = rune_tui::messages::posts(session.app());
    for key in [
        chord(KeyCode::Up, alt_sup),
        chord(KeyCode::Down, alt_sup),
        chord(KeyCode::Up, sup_only),
        chord(KeyCode::Up, shift_alt),
    ] {
        assert!(session.key(key).is_none());
        let doc = session.app().doc(doc_id).unwrap();
        assert_eq!(doc.buffer.content(), before, "{key:?} must not edit");
        assert_eq!(
            doc.journal.pos(),
            pos_before,
            "{key:?} must push no journal step"
        );
        assert_eq!(
            doc.viewport.scroll_row, scroll_before,
            "{key:?} must not scroll"
        );
        let posts_now = rune_tui::messages::posts(session.app());
        assert!(
            posts_now > posts_before,
            "{key:?} must post feedback, posts stayed at {posts_now}"
        );
        posts_before = posts_now;
    }
    assert!(
        rune_tui::messages::newest_text(session.app())
            .unwrap_or_default()
            .contains("merge:"),
        "expected the merge-key hint, got {:?}",
        rune_tui::messages::newest_text(session.app())
    );
}

/// Bare/shift arrows are the resolver's own viewport vocabulary: they must
/// move `scroll_row` without ever nagging, including at the clamp, where a
/// scroll that can go no further is silent by universal editor convention.
#[test]
fn bare_and_shift_arrows_still_scroll_without_nagging() {
    let (mut session, doc_id) = enter_two_conflict_merge();

    // Shrink the frame until the editor viewport is exactly 2 rows tall —
    // through a real resize, since every checked step's relayout re-derives
    // the viewport from the frame and would overwrite a direct `set_size`.
    let chrome_overhead = {
        let app = session.app();
        let area = Rect::new(0, 0, 80, app.frame_height);
        app.frame_height - rune_tui::layout::geometry(area, app).editor.height
    };
    assert!(session.resize(80, chrome_overhead + 2).is_none());
    assert_eq!(
        session.app().doc(doc_id).unwrap().viewport.height,
        2,
        "test setup: the editor viewport must be exactly 2 rows tall"
    );

    // `Home` is part of the resolver's own scroll vocabulary: pin the
    // viewport to the top so the row arithmetic below starts from 0 —
    // the resize above reconciled the viewport onto the caret's row.
    assert!(session.key(bare(KeyCode::Home)).is_none());
    assert_eq!(
        session.app().doc(doc_id).unwrap().viewport.scroll_row,
        DisplayRow(0)
    );

    let posts_before = rune_tui::messages::posts(session.app());
    assert!(session.key(bare(KeyCode::Down)).is_none());
    assert_eq!(
        session.app().doc(doc_id).unwrap().viewport.scroll_row,
        DisplayRow(1)
    );
    assert_eq!(
        rune_tui::messages::posts(session.app()),
        posts_before,
        "no nag on scroll"
    );

    let shift = Mods {
        shift: true,
        ..Mods::NONE
    };
    assert!(session.key(chord(KeyCode::Down, shift)).is_none());
    let scroll_after_shift = session.app().doc(doc_id).unwrap().viewport.scroll_row;
    assert!(
        scroll_after_shift > DisplayRow(1),
        "shift-down must also scroll"
    );
    assert_eq!(
        rune_tui::messages::posts(session.app()),
        posts_before,
        "no nag on scroll"
    );

    let max_row = DisplayRow(
        session
            .app_mut()
            .doc_mut(doc_id)
            .unwrap()
            .view()
            .display
            .total_rows()
            - 1,
    );
    while session.app().doc(doc_id).unwrap().viewport.scroll_row < max_row {
        assert!(session.key(bare(KeyCode::Down)).is_none());
    }
    let posts_at_clamp = rune_tui::messages::posts(session.app());
    assert!(session.key(bare(KeyCode::Down)).is_none());
    assert_eq!(
        session.app().doc(doc_id).unwrap().viewport.scroll_row,
        max_row,
        "clamp holds"
    );
    assert_eq!(
        rune_tui::messages::posts(session.app()),
        posts_at_clamp,
        "no nag at the clamp"
    );
}

#[test]
fn escape_exits_in_place_keeping_markers() {
    let (mut session, doc_id) = enter_two_conflict_merge();

    assert!(session.key(ch('b')).is_none());
    let before = session
        .app()
        .doc(doc_id)
        .unwrap()
        .buffer
        .content()
        .to_string();
    assert!(session.key(bare(KeyCode::Escape)).is_none());

    assert_eq!(session.app().merge, MergeState::Inactive);
    let doc = session.app().doc(doc_id).unwrap();
    assert_eq!(doc.buffer.content(), before, "exit in place edits nothing");
    assert!(
        doc.buffer.content().contains("<<<<<<< editor\n"),
        "the unresolved block's markers stay"
    );
    assert!(!doc.file_name().ends_with(": editor <-> disk"));
    assert!(
        rune_tui::messages::newest_text(session.app())
            .unwrap_or_default()
            .contains("1 unresolved marker block"),
        "expected the exit summary, got {:?}",
        rune_tui::messages::newest_text(session.app())
    );
}

/// The save gate is structural, not `⌘S`-only: `^W` on the dirty working
/// form arms the DirtyClose guard, and its `[S]` answer routes through the
/// same `trigger_save` ladder — with unresolved blocks it must refuse, and
/// the conflict markers must never reach the disk file.
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
        "no save may start while the resolver has unresolved blocks"
    );
    assert!(matches!(
        session.app().merge,
        MergeState::Active { doc, .. } if doc == doc_id
    ));
    assert_eq!(
        session.app().vfs.read(Path::new("/doc.md")).unwrap(),
        disk_before,
        "the conflict-marker working form must never reach the disk file"
    );
}

#[test]
fn help_lists_the_merge_bindings() {
    let help = rune_tui::help::help_markdown();
    assert!(help.contains("## Merge"), "missing merge section: {help}");
    for label in [
        "prev conflict",
        "next conflict",
        "keep editor's side",
        "keep disk's side",
        "keep both",
        "close merge",
    ] {
        assert!(help.contains(label), "missing help row {label:?}");
    }
}
