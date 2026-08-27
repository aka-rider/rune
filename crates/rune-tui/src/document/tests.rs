#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use rune_core::coords::{BufferOffset, VisualCol};
use rune_vfs::Mem;

#[test]
fn sync_reparses_once_and_is_idempotent_on_repeat_calls() {
    let mut doc = Document::new(Buffer::new("# hello\nworld\n"));
    doc.viewport.set_size(80, 24);
    let first = doc.sync();
    assert_eq!(first.display.total_rows(), 3);
    let second = doc.sync();
    assert_eq!(second.display.total_rows(), first.display.total_rows());
}

#[test]
fn sync_reconciles_the_viewport_again_after_a_reveal_driven_geometry_shrink() {
    let content = "# Doc\n\n| Name | Age |\n| :--- | ---: |\n\
                    | Alice | 30 |\n| Bob | 25 |\n\ntail\n";
    let mut doc = Document::new(Buffer::new(content));
    doc.viewport.set_size(80, 24);
    doc.focused = true;

    crate::commands::nav_scroll::scroll_line_down(&mut doc);
    let first = doc.sync();
    let scroll_after_first_sync = doc.viewport.scroll_row;

    let second = doc.sync();
    assert_eq!(
        second.display.total_rows(),
        first.display.total_rows(),
        "a second, message-free sync() changed the rendered row count"
    );
    assert_eq!(
        doc.viewport.scroll_row, scroll_after_first_sync,
        "a second, message-free sync() moved scroll_row"
    );
}

#[test]
fn sync_reconciles_the_viewport_again_after_a_reading_view_toggle() {
    let content = "# Doc\n\n| Name | Age |\n| :--- | ---: |\n\
                    | Alice | 30 |\n| Bob | 25 |\n\ntail\n";
    let mut doc = Document::new(Buffer::new(content));
    doc.viewport.set_size(80, 24);
    doc.focused = true;
    let _ = doc.sync();

    doc.read_only = ReadOnly::Reading;

    let first = doc.sync();
    let scroll_after_first_sync = doc.viewport.scroll_row;

    let second = doc.sync();
    assert_eq!(
        second.display.total_rows(),
        first.display.total_rows(),
        "a second, message-free sync() after the reading-view toggle changed \
         the rendered row count"
    );
    assert_eq!(
        doc.viewport.scroll_row, scroll_after_first_sync,
        "a second, message-free sync() after the reading-view toggle moved scroll_row"
    );
}

#[test]
fn hydrate_adopts_a_recovered_draft_even_in_reading_view() {
    let mut doc = Document::new(Buffer::new("on disk"));
    doc.read_only = ReadOnly::Reading;

    let outcome = doc.hydrate("on disk", "recovered draft", &[]);

    assert!(matches!(outcome, Hydration::Adopted));
    assert_eq!(doc.buffer.content(), "recovered draft");
    assert_eq!(doc.journal.len(), 1);
}

#[test]
fn hydrate_leaves_a_cursor_at_offset_zero_in_place() {
    let mut doc = Document::new(Buffer::new("on disk"));
    assert_eq!(doc.cursors.primary().position, BufferOffset(0));

    doc.hydrate("on disk", "a much longer recovered draft", &[]);

    assert_eq!(doc.cursors.primary().position, BufferOffset(0));
    assert_eq!(doc.cursors.primary().anchor, BufferOffset(0));
}

#[test]
fn hydrate_clamps_a_cursor_beyond_the_recovered_content() {
    let disk = "0123456789ABCDEF";
    let mut doc = Document::new(Buffer::new(disk));
    doc.cursors = CursorSet::new(doc.buffer.len());

    doc.hydrate(disk, "01234567", &[]);

    assert_eq!(
        doc.cursors.primary().position,
        BufferOffset("01234567".len())
    );
    assert_eq!(doc.cursors.primary().anchor, BufferOffset("01234567".len()));
}

#[test]
fn hydrate_lands_a_clamped_cursor_on_a_char_boundary() {
    let mut doc = Document::new(Buffer::new("aaaaaa"));
    doc.cursors = CursorSet::new(3);

    doc.hydrate("aaaaaa", "\u{e9}\u{e9}\u{e9}\u{e9}", &[]);

    let cursor = doc.cursors.primary();
    assert!(
        "\u{e9}\u{e9}\u{e9}\u{e9}".is_char_boundary(cursor.position.get()),
        "clamped position {} is not a char boundary",
        cursor.position
    );
    assert!(
        "\u{e9}\u{e9}\u{e9}\u{e9}".is_char_boundary(cursor.anchor.get()),
        "clamped anchor {} is not a char boundary",
        cursor.anchor
    );
}

fn test_cursor(position: usize, anchor: usize) -> Cursor {
    Cursor {
        position: BufferOffset(position),
        anchor: BufferOffset(anchor),
        desired_col: VisualCol(0),
        id: rune_core::cursor::CursorId::try_from(1).expect("test id is non-zero"),
    }
}

#[test]
fn hydrate_installs_the_journaled_caret_over_the_existing_one() {
    let mut doc = Document::new(Buffer::new("on disk"));
    doc.cursors = CursorSet::new(2);

    doc.hydrate("on disk", "recovered draft", &[test_cursor(9, 9)]);

    assert_eq!(doc.cursors.primary().position, BufferOffset(9));
    assert_eq!(doc.cursors.primary().anchor, BufferOffset(9));
}

#[test]
fn hydrate_installs_every_journaled_caret() {
    let mut doc = Document::new(Buffer::new("on disk"));

    doc.hydrate(
        "on disk",
        "recovered draft",
        &[
            test_cursor(2, 2),
            Cursor {
                id: rune_core::cursor::CursorId::try_from(2).expect("test id is non-zero"),
                ..test_cursor(11, 11)
            },
        ],
    );

    let positions: Vec<usize> = doc.cursors.all().iter().map(|c| c.position.get()).collect();
    assert_eq!(positions, vec![2, 11]);
}

#[test]
fn hydrate_without_a_journaled_caret_keeps_the_existing_one() {
    let mut doc = Document::new(Buffer::new("on disk"));
    doc.cursors = CursorSet::new(4);

    doc.hydrate("on disk", "recovered draft", &[]);

    assert_eq!(doc.cursors.primary().position, BufferOffset(4));
}

#[test]
fn hydrate_clamps_a_journaled_caret_past_the_end_of_the_recovered_content() {
    let mut doc = Document::new(Buffer::new("on disk"));

    doc.hydrate("on disk", "short draft", &[test_cursor(9_000, 9_000)]);

    assert_eq!(
        doc.cursors.primary().position,
        BufferOffset("short draft".len())
    );
    assert_eq!(
        doc.cursors.primary().anchor,
        BufferOffset("short draft".len())
    );
}

#[test]
fn hydrate_snaps_a_journaled_caret_off_a_char_boundary() {
    let recovered = "\u{e9}\u{e9}\u{e9}";
    let mut doc = Document::new(Buffer::new("aaaaaaaa"));

    doc.hydrate("aaaaaaaa", recovered, &[test_cursor(3, 5)]);

    let cursor = doc.cursors.primary();
    assert!(
        recovered.is_char_boundary(cursor.position.get()),
        "position {} splits a char",
        cursor.position
    );
    assert!(
        recovered.is_char_boundary(cursor.anchor.get()),
        "anchor {} splits a char",
        cursor.anchor
    );
    assert_eq!(cursor.position, BufferOffset(2));
    assert_eq!(cursor.anchor, BufferOffset(4));
}

#[test]
fn hydrate_keeps_a_cursor_offset_within_the_recovered_content() {
    let mut doc = Document::new(Buffer::new("on disk"));
    doc.cursors = CursorSet::new(3);

    doc.hydrate("on disk", "recovered draft", &[]);

    assert_eq!(doc.cursors.primary().position, BufferOffset(3));
    assert_eq!(doc.cursors.primary().anchor, BufferOffset(3));
}

#[test]
fn undoing_a_hydration_restores_the_pre_hydration_cursors_and_content() {
    let mut app = crate::app::App::new(
        Buffer::new("on disk"),
        None,
        std::sync::Arc::new(Mem::new()),
        None,
    );
    let id = app.active;
    app.doc_mut(id).unwrap().cursors = CursorSet::new(4);

    let outcome = app
        .doc_mut(id)
        .unwrap()
        .hydrate("on disk", "recovered draft", &[]);
    assert!(matches!(outcome, Hydration::Adopted));
    assert_eq!(app.doc(id).unwrap().buffer.content(), "recovered draft");

    crate::commands::edit::undo(&mut app, id);

    let doc = app.doc(id).unwrap();
    assert_eq!(
        doc.buffer.content(),
        "on disk",
        "undoing the hydration must revert the buffer to its pre-hydration content"
    );
    assert!(
        !doc.cursors.is_empty(),
        "undo must never leave an empty cursor set"
    );
    assert!(
        doc.cursors.primary().position <= BufferOffset(doc.buffer.len()),
        "the restored cursor must be in-bounds for the reverted buffer"
    );
    assert_eq!(
        doc.cursors.primary().position,
        BufferOffset(4),
        "undo must restore the actual pre-hydration cursor, not a synthesized offset-zero one"
    );
}

#[test]
fn begin_recording_reports_failure_instead_of_silently_wedging_when_not_publishing() {
    let mut doc = Document::new(Buffer::new("hello"));
    assert_eq!(doc.save_phase(), SavePhase::Idle);

    let succeeded = doc.begin_recording(1, true);

    assert!(
        !succeeded,
        "begin_recording must report failure — its caller must be able to \
         resolve the save state itself instead of assuming the transition \
         always lands"
    );
    assert_eq!(
        doc.save_phase(),
        SavePhase::Idle,
        "a rejected transition must leave the SaveState exactly as it was"
    );
}

#[test]
fn document_ids_are_distinct_and_ordered() {
    let mut app = crate::app::App::new(
        Buffer::new("a"),
        None,
        std::sync::Arc::new(Mem::new()),
        None,
    );
    let a = app.active;
    let b = app.open_document(Buffer::new("b"));
    assert_ne!(a, b);
    assert!(a < b);
}
