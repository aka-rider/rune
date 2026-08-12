//! WP4 "Done when" integration tests for the merge resolver (plan
//! `merge-user-s-changes-with-idempotent-octopus.md`): `[`/`]` navigation,
//! O/T/B accepts, the ⌘S gate, key swallowing with feedback, and the Help
//! table. Builds on `merge_entry.rs`'s fixtures, and shares its own setup
//! helpers with it via `merge_common` (review fix F9's dedupe).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod merge_common;

use std::path::Path;
use std::sync::Arc;

use rune_core::coords::DisplayRow;
use rune_db::SyncKind;
use rune_tui::app::App;
use rune_tui::db::DbBridge;
use rune_tui::document::DocumentId;
use rune_tui::keymap::{KeyCode, Mods};
use rune_tui::merge::MergeState;
use rune_tui::workspace;
use rune_vfs::{Mem, Vfs};

use merge_common::db_wiring_common::{app_with_store, publish};
use merge_common::{
    bare, ch, chord, ctrl, drain_all_ops_for, drain_one_op_for, external_write, press_key, reprobe,
    sup,
};

/// Both sides edit line 1 AND line 5 differently, with three untouched
/// context lines between — two separate conflicts under any diff engine.
const ANCESTOR: &[u8] = b"one\ntwo\nthree\nfour\nfive\n";
const THEIRS: &[u8] = b"one disk\ntwo\nthree\nfour\nfive disk\n";

/// Builds the standard two-conflict resolver session: ancestor on disk at
/// load, ours edits lines 1 and 5 by typing, theirs rewrites both on disk,
/// `^M` enters, and the resolver is Active with exactly two blocks.
fn enter_two_conflict_merge() -> (App, Arc<DbBridge>, DocumentId) {
    let mem = Mem::new();
    publish(&mem, Path::new("/doc.md"), ANCESTOR);
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(mem);

    let (mut app, bridge) = app_with_store("merge-resolver", Arc::clone(&vfs));
    let draft_id = app.active;
    workspace::open_path(&mut app, Path::new("/doc.md"));
    let doc_id = app.active;
    drain_one_op_for(&mut app, &bridge, doc_id);

    // Ours: "Xone\n...\nfiveZ\n" — one edit on each conflicting line.
    press_key(&mut app, ch('X'));
    for _ in 0..4 {
        press_key(&mut app, bare(KeyCode::Down));
    }
    press_key(&mut app, bare(KeyCode::End));
    press_key(&mut app, ch('Z'));
    assert_eq!(
        app.doc(doc_id).unwrap().buffer.content(),
        "Xone\ntwo\nthree\nfour\nfiveZ\n"
    );
    drain_all_ops_for(&mut app, &bridge, doc_id);

    external_write(vfs.as_ref(), THEIRS);
    reprobe(&mut app, &bridge, draft_id, doc_id);
    assert_eq!(app.doc(doc_id).unwrap().last_sync, Some(SyncKind::Diverged));

    app.active = doc_id;
    press_key(&mut app, ctrl('m'));
    drain_one_op_for(&mut app, &bridge, doc_id);

    let MergeState::Active { pairs, cur, .. } = &app.merge else {
        panic!("expected an active resolver, got {:?}", app.merge);
    };
    assert_eq!(pairs.len(), 2, "fixture must produce exactly two conflicts");
    assert_eq!(*cur, 0);
    (app, bridge, doc_id)
}

fn current_block(app: &App) -> usize {
    let MergeState::Active { cur, .. } = &app.merge else {
        panic!("resolver not active");
    };
    *cur
}

#[test]
fn bracket_navigation_cycles_and_skips_resolved_blocks() {
    let (mut app, _bridge, _doc) = enter_two_conflict_merge();

    press_key(&mut app, ch(']'));
    assert_eq!(current_block(&app), 1);
    press_key(&mut app, ch(']'));
    assert_eq!(current_block(&app), 0, "next wraps around");
    press_key(&mut app, ch('['));
    assert_eq!(current_block(&app), 1, "prev wraps around");
    press_key(&mut app, ch('['));
    assert_eq!(current_block(&app), 0);

    // Resolve block 0; navigation must now skip it from either direction.
    press_key(&mut app, ch('b'));
    assert_eq!(current_block(&app), 1, "accept advances to next unresolved");
    press_key(&mut app, ch(']'));
    assert_eq!(current_block(&app), 1, "the resolved block is skipped");
    press_key(&mut app, ch('['));
    assert_eq!(current_block(&app), 1);
}

#[test]
fn ours_and_theirs_collapse_blocks_to_exact_bytes_one_journal_step_each() {
    let (mut app, _bridge, doc_id) = enter_two_conflict_merge();
    let saved_name = app.doc(doc_id).unwrap().file_name().to_string();
    assert!(saved_name.ends_with(": editor <-> disk"));

    let pos_before = app.doc(doc_id).unwrap().journal.pos();
    press_key(&mut app, ch('o'));
    let doc = app.doc(doc_id).unwrap();
    assert_eq!(doc.journal.pos(), pos_before + 1, "O is one journal step");
    assert!(doc.buffer.content().starts_with("Xone\ntwo\n"));
    assert!(
        !doc.buffer.content().starts_with("<<<<<<<"),
        "block 1 must be collapsed: {:?}",
        doc.buffer.content()
    );

    let pos_before = app.doc(doc_id).unwrap().journal.pos();
    press_key(&mut app, ch('t'));
    let doc = app.doc(doc_id).unwrap();
    assert_eq!(doc.journal.pos(), pos_before + 1, "T is one journal step");
    assert_eq!(
        doc.buffer.content(),
        "Xone\ntwo\nthree\nfour\nfive disk\n",
        "O kept ours on block 1, T kept theirs on block 2"
    );

    // Decision 13: resolving the last hunk exits in place.
    assert_eq!(app.merge, MergeState::Inactive);
    assert!(
        !app.doc(doc_id)
            .unwrap()
            .file_name()
            .ends_with(": editor <-> disk"),
        "title must revert on exit"
    );
    assert!(
        rune_tui::messages::newest_text(&app)
            .unwrap_or_default()
            .contains("merge complete"),
        "expected the merge-complete status, got {:?}",
        rune_tui::messages::newest_text(&app)
    );
}

#[test]
fn both_strips_markers_and_keeps_both_sides_as_one_edit() {
    let (mut app, _bridge, doc_id) = enter_two_conflict_merge();

    let pos_before = app.doc(doc_id).unwrap().journal.pos();
    press_key(&mut app, ch('b'));

    let doc = app.doc(doc_id).unwrap();
    assert_eq!(doc.journal.pos(), pos_before + 1, "B is one journal step");
    assert!(
        doc.buffer.content().starts_with("Xone\n\none disk\ntwo\n"),
        "B keeps ours then theirs with no marker lines: {:?}",
        doc.buffer.content()
    );
    let MergeState::Active { pairs, .. } = &app.merge else {
        panic!("resolver still active after resolving 1 of 2");
    };
    assert!(pairs[0].block.resolved);
    assert!(!pairs[1].block.resolved);
    assert_eq!(
        doc.buffer.content().matches("<<<<<<<").count(),
        1,
        "only the unresolved block's markers remain"
    );
}

#[test]
fn all_both_resolution_leaves_zero_marker_bytes() {
    let (mut app, _bridge, doc_id) = enter_two_conflict_merge();

    press_key(&mut app, ch('b'));
    press_key(&mut app, ch('b'));

    assert_eq!(app.merge, MergeState::Inactive, "all resolved exits merge");
    let content = app.doc(doc_id).unwrap().buffer.content().to_string();
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
        rune_tui::messages::newest_text(&app)
            .unwrap_or_default()
            .contains("merge complete"),
        "expected the merge-complete status, got {:?}",
        rune_tui::messages::newest_text(&app)
    );
}

#[test]
fn save_is_refused_while_unresolved_and_allowed_after() {
    let (mut app, _bridge, doc_id) = enter_two_conflict_merge();

    let before = app.doc(doc_id).unwrap().buffer.content().to_string();
    press_key(&mut app, sup('s'));
    assert!(
        rune_tui::messages::newest_text(&app)
            .unwrap_or_default()
            .contains("conflict(s) to resolve"),
        "expected the save refusal, got {:?}",
        rune_tui::messages::newest_text(&app)
    );
    assert_eq!(
        app.doc(doc_id).unwrap().buffer.content(),
        before,
        "a refused save must not touch the buffer"
    );

    press_key(&mut app, ch('o'));
    press_key(&mut app, ch('t'));
    assert_eq!(app.merge, MergeState::Inactive);
    press_key(&mut app, sup('s'));
    assert!(
        !rune_tui::messages::newest_text(&app)
            .unwrap_or_default()
            .contains("conflict(s) to resolve"),
        "after full resolution ⌘S must not be refused, got {:?}",
        rune_tui::messages::newest_text(&app)
    );
}

#[test]
fn unbound_keys_are_swallowed_with_feedback_while_resolving() {
    let (mut app, _bridge, doc_id) = enter_two_conflict_merge();

    let before = app.doc(doc_id).unwrap().buffer.content().to_string();
    let pos_before = app.doc(doc_id).unwrap().journal.pos();
    for key in [
        ch('x'),
        ch('7'),
        bare(KeyCode::Enter),
        bare(KeyCode::Backspace),
    ] {
        press_key(&mut app, key);
        assert_eq!(
            app.doc(doc_id).unwrap().buffer.content(),
            before,
            "{key:?} must not reach the working form"
        );
        assert!(
            rune_tui::messages::newest_text(&app)
                .unwrap_or_default()
                .contains("merge:"),
            "{key:?} must be swallowed WITH feedback, got {:?}",
            rune_tui::messages::newest_text(&app)
        );
    }
    assert_eq!(app.doc(doc_id).unwrap().journal.pos(), pos_before);
}

/// Issue #54: `⌥⌘↑`/`⌥⌘↓` (`AddCursorAbove`/`AddCursorBelow`), bare `⌘↑`, and
/// `⇧⌥↑` (`CloneLineUp`) are ordinary editor commands with no meaning while
/// the resolver owns the keyboard, and must be refused out loud rather than
/// silently re-keyed into a one-row scroll.
#[test]
fn modifier_arrow_chords_are_refused_with_feedback_while_resolving() {
    let (mut app, _bridge, doc_id) = enter_two_conflict_merge();

    let before = app.doc(doc_id).unwrap().buffer.content().to_string();
    let pos_before = app.doc(doc_id).unwrap().journal.pos();
    let scroll_before = app.doc(doc_id).unwrap().viewport.scroll_row;
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
    let mut posts_before = rune_tui::messages::posts(&app);
    for key in [
        chord(KeyCode::Up, alt_sup),
        chord(KeyCode::Down, alt_sup),
        chord(KeyCode::Up, sup_only),
        chord(KeyCode::Up, shift_alt),
    ] {
        press_key(&mut app, key);
        let doc = app.doc(doc_id).unwrap();
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
        let posts_now = rune_tui::messages::posts(&app);
        assert!(
            posts_now > posts_before,
            "{key:?} must post feedback, posts stayed at {posts_now}"
        );
        posts_before = posts_now;
    }
    assert!(
        rune_tui::messages::newest_text(&app)
            .unwrap_or_default()
            .contains("merge:"),
        "expected the merge-key hint, got {:?}",
        rune_tui::messages::newest_text(&app)
    );
}

/// Bare/shift arrows are the resolver's own viewport vocabulary: they must
/// move `scroll_row` without ever nagging, including at the clamp, where a
/// scroll that can go no further is silent by universal editor convention.
#[test]
fn bare_and_shift_arrows_still_scroll_without_nagging() {
    let (mut app, _bridge, doc_id) = enter_two_conflict_merge();
    app.active_doc_mut().viewport.set_size(80, 2);

    let posts_before = rune_tui::messages::posts(&app);
    press_key(&mut app, bare(KeyCode::Down));
    assert_eq!(app.doc(doc_id).unwrap().viewport.scroll_row, DisplayRow(1));
    assert_eq!(
        rune_tui::messages::posts(&app),
        posts_before,
        "no nag on scroll"
    );

    let shift = Mods {
        shift: true,
        ..Mods::NONE
    };
    press_key(&mut app, chord(KeyCode::Down, shift));
    let scroll_after_shift = app.doc(doc_id).unwrap().viewport.scroll_row;
    assert!(
        scroll_after_shift > DisplayRow(1),
        "shift-down must also scroll"
    );
    assert_eq!(
        rune_tui::messages::posts(&app),
        posts_before,
        "no nag on scroll"
    );

    let max_row = DisplayRow(app.doc_mut(doc_id).unwrap().view().display.total_rows() - 1);
    while app.doc(doc_id).unwrap().viewport.scroll_row < max_row {
        press_key(&mut app, bare(KeyCode::Down));
    }
    let posts_at_clamp = rune_tui::messages::posts(&app);
    press_key(&mut app, bare(KeyCode::Down));
    assert_eq!(
        app.doc(doc_id).unwrap().viewport.scroll_row,
        max_row,
        "clamp holds"
    );
    assert_eq!(
        rune_tui::messages::posts(&app),
        posts_at_clamp,
        "no nag at the clamp"
    );
}

#[test]
fn escape_exits_in_place_keeping_markers() {
    let (mut app, _bridge, doc_id) = enter_two_conflict_merge();

    press_key(&mut app, ch('b'));
    let before = app.doc(doc_id).unwrap().buffer.content().to_string();
    press_key(&mut app, bare(KeyCode::Escape));

    assert_eq!(app.merge, MergeState::Inactive);
    let doc = app.doc(doc_id).unwrap();
    assert_eq!(doc.buffer.content(), before, "exit in place edits nothing");
    assert!(
        doc.buffer.content().contains("<<<<<<< editor\n"),
        "the unresolved block's markers stay"
    );
    assert!(!doc.file_name().ends_with(": editor <-> disk"));
    assert!(
        rune_tui::messages::newest_text(&app)
            .unwrap_or_default()
            .contains("1 unresolved marker block"),
        "expected the exit summary, got {:?}",
        rune_tui::messages::newest_text(&app)
    );
}

/// The save gate is structural, not `⌘S`-only: `^W` on the dirty working
/// form arms the DirtyClose guard, and its `[S]` answer routes through the
/// same `trigger_save` ladder — with unresolved blocks it must refuse, and
/// the conflict markers must never reach the disk file.
#[test]
fn dirty_close_guard_save_during_unresolved_merge_refuses_and_writes_nothing() {
    let (mut app, _bridge, doc_id) = enter_two_conflict_merge();
    let disk_before = app.vfs.read(Path::new("/doc.md")).unwrap();

    press_key(&mut app, ctrl('w'));
    assert!(
        matches!(
            &app.guard,
            Some(prompt) if prompt.kind == rune_tui::guard::GuardKind::DirtyClose
        ),
        "closing the dirty working form must arm the DirtyClose guard"
    );

    press_key(&mut app, ch('s'));

    assert!(
        rune_tui::messages::newest_text(&app)
            .unwrap_or_default()
            .contains("conflict(s) to resolve"),
        "expected the merge save refusal, got {:?}",
        rune_tui::messages::newest_text(&app)
    );
    assert!(app.doc(doc_id).is_some(), "the document must stay open");
    assert!(
        !app.doc(doc_id).unwrap().save_in_flight(),
        "no save may start while the resolver has unresolved blocks"
    );
    assert!(matches!(
        app.merge,
        MergeState::Active { doc, .. } if doc == doc_id
    ));
    assert_eq!(
        app.vfs.read(Path::new("/doc.md")).unwrap(),
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
