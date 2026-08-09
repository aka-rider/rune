#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use rune_vfs::Mem;

#[test]
fn sync_reparses_once_and_is_idempotent_on_repeat_calls() {
    let mut doc = Document::new(Buffer::new("# hello\nworld\n"));
    doc.viewport.set_size(80, 24);
    let first = doc.sync();
    // "# hello" + "world" + the trailing empty line from the final \n.
    assert_eq!(first.display.total_rows(), 3);
    let second = doc.sync();
    assert_eq!(second.display.total_rows(), first.display.total_rows());
}

/// The `TODO-fuzz-sync-idempotent-table-scroll.md` regression, pinned
/// directly against `Document::sync` rather than only through the
/// checked-in fuzz replay (`crates/rune-fuzz/repros/sync-idempotent-
/// 04.rune`): `scroll_line_down` (Independent-mode `ctrl+down`) snaps
/// the cursor INTO a boxed table, which is itself a `RevealGrant::
/// Decide` policy (`rune_md::emit::table::emit_table`) — collapsing the
/// table from its bordered layout to bare source lines shrinks
/// `total_rows` out from under the `Viewport::reconcile` call that just
/// ran against the PRE-collapse geometry, leaving `scroll_row` outside
/// the settled scrolloff band. A second, message-free `sync()` must not
/// see this catch up on its own — `sync()` itself must already be a
/// fixpoint.
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

/// Plan WP5: the `⌃P` toggle changes reveal-driven geometry with the
/// cursor STATIONARY (unlike the reveal-driven-shrink case above, where
/// `scroll_to_cursor` moves the cursor into the collapsing table) — the
/// first `view()` call's `set_reveal_mode` transition already marks
/// `DocMachine` dirty before `scroll_to_cursor` ever runs, so this is
/// strictly easier than the case `sync`'s fixpoint sequence was built for.
/// Pinned the same way: sync, flip `read_only` (the toggle's own mint
/// site), sync twice more, and compare.
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

/// Plan WP4: `hydrate` stays exempt from `read_only` even under
/// `ReadOnly::Reading` — a reading-view toggle must never cost the user a
/// recovered draft. Pinned against the public `Document::hydrate` entry
/// point, not the mint sites that call it.
#[test]
fn hydrate_adopts_a_recovered_draft_even_in_reading_view() {
    let mut doc = Document::new(Buffer::new("on disk"));
    doc.read_only = ReadOnly::Reading;

    let outcome = doc.hydrate("on disk", "recovered draft");

    assert!(matches!(outcome, Hydration::Adopted));
    assert_eq!(doc.buffer.content(), "recovered draft");
    assert_eq!(doc.journal.len(), 1);
}

#[test]
fn hydrate_leaves_a_cursor_at_offset_zero_in_place() {
    let mut doc = Document::new(Buffer::new("on disk"));
    assert_eq!(doc.cursors.primary().position, 0);

    doc.hydrate("on disk", "a much longer recovered draft");

    assert_eq!(doc.cursors.primary().position, 0);
    assert_eq!(doc.cursors.primary().anchor, 0);
}

#[test]
fn hydrate_clamps_a_cursor_beyond_the_recovered_content() {
    let disk = "0123456789ABCDEF";
    let mut doc = Document::new(Buffer::new(disk));
    doc.cursors = CursorSet::new(doc.buffer.len());

    doc.hydrate(disk, "01234567");

    assert_eq!(doc.cursors.primary().position, "01234567".len());
    assert_eq!(doc.cursors.primary().anchor, "01234567".len());
}

#[test]
fn hydrate_lands_a_clamped_cursor_on_a_char_boundary() {
    let mut doc = Document::new(Buffer::new("aaaaaa"));
    doc.cursors = CursorSet::new(3);

    doc.hydrate("aaaaaa", "\u{e9}\u{e9}\u{e9}\u{e9}");

    let cursor = doc.cursors.primary();
    assert!(
        "\u{e9}\u{e9}\u{e9}\u{e9}".is_char_boundary(cursor.position),
        "clamped position {} is not a char boundary",
        cursor.position
    );
    assert!(
        "\u{e9}\u{e9}\u{e9}\u{e9}".is_char_boundary(cursor.anchor),
        "clamped anchor {} is not a char boundary",
        cursor.anchor
    );
}

#[test]
fn hydrate_keeps_a_cursor_offset_within_the_recovered_content() {
    let mut doc = Document::new(Buffer::new("on disk"));
    doc.cursors = CursorSet::new(3);

    doc.hydrate("on disk", "recovered draft");

    assert_eq!(doc.cursors.primary().position, 3);
    assert_eq!(doc.cursors.primary().anchor, 3);
}

#[test]
fn document_ids_are_distinct_and_ordered() {
    // Mints two REAL ids the same way production code does — through
    // `App`, never a raw-number constructor.
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
