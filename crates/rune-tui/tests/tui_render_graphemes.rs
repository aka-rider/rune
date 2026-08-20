//! Headless render assertions on a `TestBackend`, using the
//! `Mem` vfs — grapheme-cluster cells (ZWJ sequences, skin-tone modifiers,
//! variation selectors, and the zero-width-rune width policy). This is the
//! 500-line-budget split of the original `tui_render_text.rs`: control-safe
//! glyphs and tab expansion live in the sibling `tui_render_text.rs`;
//! conceal/styling/status-line/Cell-grid checks live in
//! `tui_render_basics.rs`, degenerate backend sizes and `blit`'s own
//! clipping in `tui_render_bounds.rs`, and tables/the focus caret gate in
//! `tui_render_focus.rs`. The runtime loop itself is NOT exercised here
//! (test the pure update/view paths headlessly; do NOT spawn real
//! terminals in tests) — every test drives `App`/`render::draw` directly.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod tui_render_common;

use ratatui::buffer::CellWidth;
use rune_tui::render;

use tui_render_common::{
    EDITOR_LEFT_COL, EDITOR_TOP_ROW, HEIGHT, WIDTH, app_for, full_text, render_to_test_backend,
    row_text,
};

/// Regression for the grapheme-cluster cell builder (caught by
/// screen-capture testing): a ZWJ family emoji (7 codepoints
/// joined by U+200D) must render as exactly ONE `Cell` — never one `Cell`
/// per codepoint, which corrupted the terminal output (module docs,
/// `push_grapheme_cells`) — and the buffer's own bytes must stay verbatim:
/// only the DISPLAY grouping changes, never the
/// underlying content.
#[test]
fn zwj_family_emoji_renders_as_one_cell_and_buffer_bytes_round_trip() {
    let family = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{200D}\u{1F466}"; // 👨‍👩‍👧‍👦
    let content = format!("{family}\n");
    let session = app_for(&content, 0, true);
    let app = session.app();

    assert_eq!(
        app.active_doc().buffer.content(),
        content,
        "buffer bytes must round-trip verbatim across the ZWJ sequence"
    );

    let view = app.active_doc().view.as_ref().expect("synced view");
    let rows = render::build_rows(app, app.active_doc(), Some(app.active), view);
    let first_row = rows.first().expect("at least one row");
    assert_eq!(
        first_row.len(),
        1,
        "a ZWJ grapheme cluster must render as exactly one Cell: {first_row:?}"
    );
    assert_eq!(
        first_row[0].text, family,
        "the cell's text must be the whole grapheme cluster verbatim"
    );
    assert_eq!(first_row[0].buf_offset, Some(0));
}

/// Same regression, for a skin-tone-modified emoji (base codepoint + a
/// Fitzpatrick modifier codepoint — 2 codepoints, one grapheme cluster).
#[test]
fn skin_tone_modifier_emoji_renders_as_one_cell_and_buffer_bytes_round_trip() {
    let wave = "\u{1F44B}\u{1F3FD}"; // 👋🏽 (waving hand + medium skin tone)
    let content = format!("{wave}\n");
    let session = app_for(&content, 0, true);
    let app = session.app();

    assert_eq!(
        app.active_doc().buffer.content(),
        content,
        "buffer bytes must round-trip verbatim across the skin-tone modifier"
    );

    let view = app.active_doc().view.as_ref().expect("synced view");
    let rows = render::build_rows(app, app.active_doc(), Some(app.active), view);
    let first_row = rows.first().expect("at least one row");
    assert_eq!(
        first_row.len(),
        1,
        "a skin-tone-modified emoji must render as exactly one Cell: {first_row:?}"
    );
    assert_eq!(first_row[0].text, wave);
    assert_eq!(first_row[0].buf_offset, Some(0));
}

/// Regression for `blit`'s continuation-cell reset (the other half of the
/// ZWJ fix): a wide `Cell` must leave every column it covers, beyond its
/// own first, properly BLANK in the real `ratatui::buffer::Buffer` — never
/// carrying whatever a neighboring `Cell`'s content would otherwise be,
/// which is what let a ZWJ sequence's later codepoints corrupt the row
/// (ratatui's own diffing silently skips re-examining a wide cell's
/// covered columns; module docs, `blit`).
#[test]
fn wide_cell_leaves_a_blank_continuation_column_in_the_real_backend() {
    let family = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{200D}\u{1F466}";
    let content = format!("{family} x\n");
    let session = app_for(&content, 0, true);
    let app = session.app();

    let view = app.active_doc().view.as_ref().expect("synced view");
    let rows = render::build_rows(app, app.active_doc(), Some(app.active), view);
    let first_row = rows.first().expect("at least one row");
    let family_cell = first_row.first().expect("family cell present");
    assert_eq!(family_cell.text, family);
    let width = family_cell.width;
    assert!(width > 1, "family emoji must occupy more than one column");

    let buf = render_to_test_backend(app);
    for dx in 1..u16::from(width) {
        let x = EDITOR_LEFT_COL + dx;
        let cell = buf.cell((x, EDITOR_TOP_ROW)).expect("cell in bounds");
        assert_eq!(
            cell.symbol(),
            " ",
            "continuation column {dx} of the wide grapheme must be blank, got {:?}",
            cell.symbol()
        );
    }
}

/// Guard for the width chokepoint's own invariant (`rune_syntax::wrap::
/// grapheme_width`'s doc comment): rune's width for a symbol must equal
/// what ratatui derives for that same symbol, over a corpus covering every
/// class of cluster known to have diverged — plain ASCII/CJK single runes,
/// an NFD accent cluster, a ZWJ family, a skin-tone modifier, a base char
/// plus `U+FE0F`/`U+FE0E` (variation selectors), a regional-indicator flag
/// pair, a keycap, halfwidth katakana + dakuten, and the reported
/// `🤖ིྀ`-style cluster. `rune-syntax` stays terminal-free and cannot depend
/// on ratatui to assert this itself, so the equality is pinned here, in
/// `rune-tui`, which already depends on both.
#[test]
fn grapheme_width_agrees_with_ratatuis_own_cell_width_derivation() {
    let corpus: &[(&str, &str)] = &[
        ("plain ASCII", "a"),
        ("CJK", "\u{6c49}"),                 // 汉
        ("NFD accent cluster", "e\u{0301}"), // e + combining acute
        (
            "ZWJ family",
            "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{200D}\u{1F466}",
        ),
        ("skin-tone modifier", "\u{1F44B}\u{1F3FD}"), // 👋🏽
        ("heart + FE0F", "\u{2764}\u{FE0F}"),         // ❤️
        ("lightning + FE0E", "\u{26A1}\u{FE0E}"),     // ⚡︎
        ("regional-indicator flag pair", "\u{1F1FA}\u{1F1F8}"), // 🇺🇸
        ("keycap", "1\u{FE0F}\u{20E3}"),              // 1️⃣
        ("halfwidth katakana + dakuten", "\u{FF76}\u{FF9E}"), // ｶﾞ
        (
            "reported robot + Tibetan vowel signs",
            "\u{1F916}\u{0F72}\u{0F80}",
        ),
        ("lone halfwidth dakuten", "\u{FF9E}"),
        ("lone combining acute accent", "\u{0301}"),
        ("lone zero-width joiner", "\u{200D}"),
        (
            "lone variation selector-16 (emoji presentation)",
            "\u{FE0F}",
        ),
        ("lone variation selector-15 (text presentation)", "\u{FE0E}"),
        ("lone zero-width space", "\u{200B}"),
    ];

    for (label, cluster) in corpus {
        let rune_width = rune_syntax::wrap::grapheme_width(cluster);
        let ratatui_width = usize::from(cluster.cell_width());
        assert_eq!(
            rune_width, ratatui_width,
            "{label} ({cluster:?}): rune grapheme_width={rune_width}, ratatui cell_width={ratatui_width}"
        );
    }
}

/// End-to-end sibling to the corpus guard above, reusing the `TestBackend`
/// harness from `wide_cell_leaves_a_blank_continuation_column_in_the_real_
/// backend`: `❤️` (base + `U+FE0F`, ratatui width 2) followed by another
/// glyph must not swallow that glyph — before the fix, `grapheme_width`
/// under-measured this cluster at 1, `blit` wrote it at width 1 and skipped
/// its continuation-reset loop, and `BufferDiff` then read the emoji's OWN
/// `cell_width() == 2` and skipped the very column the next glyph landed
/// on, so it never reached the real terminal buffer.
#[test]
fn variation_selector_emoji_does_not_swallow_the_following_glyph() {
    let heart = "\u{2764}\u{FE0F}"; // ❤️
    let content = format!("{heart}x\n");
    let session = app_for(&content, 0, true);
    let app = session.app();

    let view = app.active_doc().view.as_ref().expect("synced view");
    let rows = render::build_rows(app, app.active_doc(), Some(app.active), view);
    let first_row = rows.first().expect("at least one row");
    let heart_cell = first_row.first().expect("heart cell present");
    assert_eq!(heart_cell.text, heart);
    assert_eq!(
        usize::from(heart_cell.width),
        heart.cell_width() as usize,
        "the Cell's own width must match ratatui's derivation for the same symbol"
    );

    let buf = render_to_test_backend(app);
    let text = full_text(&buf, HEIGHT, WIDTH);
    assert!(
        text.contains('x'),
        "the glyph following the FE0F-presented heart must reach the real backend buffer:\n{text}"
    );
}

/// The decided policy (superseding the FORMER documented exception this
/// test used to pin, `lone_zero_width_cluster_reserves_width_one_though_
/// ratatui_derives_zero`): a LONE (single-`char`) cluster that ratatui
/// itself derives width 0 for — a bare combining mark with no base, a stray
/// ZWJ, a lone variation selector, a lone zero-width space — now derives
/// width 0 in rune too (`control_aware_width`'s doc), the SAME number as
/// ratatui, with no exception left to admit in `blit`'s own strict-mode
/// `assert_invariant`. This test pins that agreement explicitly, and proves
/// the whole render path (`blit`'s strict-mode assert live in this
/// `cfg(test)` build) still renders it without panicking despite the
/// `Cell` occupying no screen column of its own.
#[test]
fn lone_zero_width_cluster_derives_zero_width_matching_ratatui() {
    let halfwidth_dakuten = '\u{FF9E}'; // agrees with ratatui at width 1 (control_aware_width's own doc) — the control case
    let lone_zero_width: &[(&str, char)] = &[
        ("combining acute accent", '\u{0301}'),
        ("zero-width joiner", '\u{200D}'),
        ("variation selector-16 (emoji presentation)", '\u{FE0F}'),
        ("variation selector-15 (text presentation)", '\u{FE0E}'),
        ("zero-width space", '\u{200B}'),
    ];

    for (label, ch) in lone_zero_width {
        assert_ne!(
            *ch, halfwidth_dakuten,
            "sanity: the halfwidth-dakuten control case must not appear in this corpus"
        );
        assert_eq!(
            usize::from(ch.to_string().cell_width()),
            0,
            "{label} ({ch:?}): expected ratatui to derive width 0 for this rune — \
             if this now fails, the corpus assumption this test relies on no longer holds"
        );

        // The rune must lead its line, with no preceding base character —
        // a combining mark (or ZWJ/variation-selector, which also only
        // attach BACKWARD to a preceding grapheme, per UAX #29 GB9) fuses
        // with a preceding base into ONE multi-rune cluster instead of
        // standing alone, which is the already-agreeing case the corpus
        // test above covers, not this one.
        let content = format!("{ch}ab\n");
        let session = app_for(&content, 0, true);
        let app = session.app();

        assert_eq!(
            app.active_doc().buffer.content(),
            content,
            "{label}: buffer bytes must round-trip verbatim"
        );

        let view = app.active_doc().view.as_ref().expect("synced view");
        let rows = render::build_rows(app, app.active_doc(), Some(app.active), view);
        let cell = rows
            .first()
            .and_then(|row| row.iter().find(|c| c.text.chars().eq(std::iter::once(*ch))))
            .expect("expected a Cell carrying the lone rune verbatim");
        assert_eq!(
            cell.width, 0,
            "{label}: rune now derives width 0 for a lone zero-width rune, matching ratatui"
        );

        // The end-to-end render must not panic — this exercises `blit`'s
        // own strict-mode `assert_invariant`, now satisfied WITHOUT any
        // exception, in a real cfg(test) build (the strict-invariants gate
        // is armed here).
        let buf = render_to_test_backend(app);
        let text = full_text(&buf, HEIGHT, WIDTH);
        assert!(
            text.contains('a') && text.contains('b'),
            "{label}: the surrounding glyphs must still reach the real backend buffer:\n{text}"
        );
    }
}

/// The actual user-visible symptom the width-0 policy above exists to fix:
/// before it, a lone zero-width rune reserved a real screen column it
/// ratatui itself never draws, so `a`/`b` after it landed one column too far
/// right on a real terminal even though rune's own `Cell` accounting looked
/// internally consistent. Pins the fix directly against the BACKEND column
/// — not just the `Cell.width` number above — for every rune in the same
/// corpus: `a` must land at the editor's own first content column
/// (`EDITOR_LEFT_COL`), exactly where ratatui would place it for `"ab\n"`
/// with no leading rune at all, never one column further right.
#[test]
fn text_after_a_lone_zero_width_rune_starts_at_ratatuis_own_column() {
    let lone_zero_width: &[(&str, char)] = &[
        ("combining acute accent", '\u{0301}'),
        ("zero-width joiner", '\u{200D}'),
        ("variation selector-16 (emoji presentation)", '\u{FE0F}'),
        ("variation selector-15 (text presentation)", '\u{FE0E}'),
        ("zero-width space", '\u{200B}'),
    ];

    for (label, ch) in lone_zero_width {
        let content = format!("{ch}ab\n");
        let session = app_for(&content, 0, true);
        let app = session.app();

        let buf = render_to_test_backend(app);
        let row = row_text(&buf, EDITOR_TOP_ROW, WIDTH);
        let a_col = row
            .chars()
            .position(|c| c == 'a')
            .expect("'a' must reach the real backend buffer");
        assert_eq!(
            a_col, EDITOR_LEFT_COL as usize,
            "{label}: 'a' must start at the editor's own first content column \
             ({EDITOR_LEFT_COL}), not one column further right — got column {a_col}:\n{row:?}"
        );
    }

    // Control: the SAME corpus check with no leading rune at all lands on
    // the identical column — proves `EDITOR_LEFT_COL` itself is the right
    // baseline, not a number picked to make the assertion above pass.
    let session = app_for("ab\n", 0, true);
    let app = session.app();
    let buf = render_to_test_backend(app);
    let row = row_text(&buf, EDITOR_TOP_ROW, WIDTH);
    let a_col = row
        .char_indices()
        .find(|(_, c)| *c == 'a')
        .map(|(byte_idx, _)| row[..byte_idx].chars().count())
        .expect("'a' must reach the real backend buffer");
    assert_eq!(a_col, EDITOR_LEFT_COL as usize);
}
