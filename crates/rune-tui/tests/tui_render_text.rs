//! WP5 done-when: headless render assertions on a `TestBackend`, using the
//! `Mem` vfs — control-safe glyphs, tab expansion, and grapheme-cluster
//! cells. TODO.md's 500-line budget split of the original `tui_render.rs`: conceal/
//! styling/status-line/Cell-grid checks live in `tui_render_basics.rs`,
//! degenerate backend sizes and `blit`'s own clipping in
//! `tui_render_bounds.rs`, and tables/the focus caret gate in
//! `tui_render_focus.rs`. The runtime loop itself is NOT exercised here
//! (plan: "test the pure update/view paths headlessly; do NOT spawn real
//! terminals in tests") — every test drives `App`/`render::draw` directly.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod tui_render_common;

use ratatui::buffer::CellWidth;
use rune_tui::render;

use tui_render_common::{
    EDITOR_LEFT_COL, EDITOR_TOP_ROW, HEIGHT, WIDTH, app_for, caret_column, full_text,
    render_to_test_backend, row_text,
};

/// Regression for the control-safe cell builder: `\r` (from a CRLF file —
/// it must stay in the buffer verbatim) must never
/// become a `Cell`, and rendering it must not panic. Before the fix, a raw
/// `\r` reached `ratatui::buffer::Cell::set_char`, and `cell_width()`
/// `debug_assert!`s on any single-byte ASCII control character reaching a
/// cell — this test IS the regression check: merely rendering CRLF content
/// without panicking is the assertion.
#[test]
fn crlf_line_endings_render_without_panicking_and_leave_no_control_chars_in_cells() {
    let content = "ab\r\ncd\r\n";
    let session = app_for(content, 0, true);
    let app = session.app();

    let buf = render_to_test_backend(app);
    // `full_text` itself joins rows with '\n' as a formatting separator, so
    // only '\r' is checked here — a real leaked '\n' cell is instead caught
    // below, directly on the `Cell` grid (which has no such separator).
    let text = full_text(&buf, HEIGHT, WIDTH);
    assert!(
        !text.contains('\r'),
        "a raw CR must never reach the terminal buffer:\n{text:?}"
    );
    assert!(text.contains("ab"), "expected 'ab' visible:\n{text}");
    assert!(text.contains("cd"), "expected 'cd' visible:\n{text}");

    let view = app.active_doc().view.as_ref().expect("synced view");
    let rows = render::build_rows(app, app.active_doc(), Some(app.active), view);
    for row in &rows {
        for cell in row {
            assert!(
                !matches!(cell.text.as_str(), "\r" | "\n"),
                "a raw CR/LF must never become a Cell: {cell:?}"
            );
        }
    }
}

/// Sibling to the CRLF regression above, for the case CRLF does NOT cover:
/// a LONE `\r` (no paired `\n`) is ordinary mid-line content to the buffer
/// (never a line break), so `"ab\rcd\r"` is a single
/// buffer line containing two literal `\r` bytes. This must render without
/// panicking and without ever letting a raw `\r` reach a `Cell`, exactly
/// like the CRLF case — the render-layer contract (the user's
/// bytes stay in the buffer verbatim; the control-safe cell builder maps
/// a control byte to a placeholder glyph, never a raw `Cell`) does not
/// distinguish a CR paired with LF from one that stands alone.
#[test]
fn lone_cr_line_endings_render_without_panicking_and_leave_no_control_chars_in_cells() {
    let content = "ab\rcd\r";
    let session = app_for(content, 0, true);
    let app = session.app();

    let buf = render_to_test_backend(app);
    let text = full_text(&buf, HEIGHT, WIDTH);
    assert!(
        !text.contains('\r'),
        "a raw CR must never reach the terminal buffer:\n{text:?}"
    );
    assert!(text.contains("ab"), "expected 'ab' visible:\n{text}");
    assert!(text.contains("cd"), "expected 'cd' visible:\n{text}");

    let view = app.active_doc().view.as_ref().expect("synced view");
    let rows = render::build_rows(app, app.active_doc(), Some(app.active), view);
    for row in &rows {
        for cell in row {
            assert!(
                !matches!(cell.text.as_str(), "\r" | "\n"),
                "a raw CR/LF must never become a Cell: {cell:?}"
            );
        }
    }
}

/// Regression for the unified width chokepoint: a tab mid-line must expand
/// to the SAME next-4-stop column both `render::segment_cells` and
/// `WrapSnapshot::visual_col` compute, so the caret lands on the character
/// after the tab, not one column short of it. Before the fix, the render
/// side treated a tab as width 1 (via `control_aware_width` alone) while
/// wrap's `visual_col` used `rune_width_with_tab`'s 4-stop math — the caret
/// landed mid-tab-expansion instead of on "c".
#[test]
fn tab_caret_column_agrees_with_wrap_visual_col() {
    let content = "ab\tcd\n";
    let cursor_offset = 3; // byte offset of 'c', right after the tab
    let session = app_for(content, cursor_offset, true);
    let app = session.app();

    let view = app.active_doc().view.as_ref().expect("synced view");
    let buffer_point = app.active_doc().buffer.offset_to_line_col(cursor_offset);
    let syntax_point = view.syntax.buffer_to_syntax(buffer_point);
    let wrap_point = view.wrap.syntax_to_wrap(syntax_point);
    let expected_visual_col = view
        .wrap
        .visual_col(content, wrap_point.row, wrap_point.col);
    assert_eq!(
        expected_visual_col, 4,
        "a tab starting at column 2 must expand to the next 4-stop (column 4)"
    );

    let buf = render_to_test_backend(app);
    // Skip the center block's left AND right border columns (plan gotcha
    // 10) before comparing against the editor-relative text.
    let text: String = row_text(&buf, EDITOR_TOP_ROW, WIDTH)
        .chars()
        .skip(EDITOR_LEFT_COL as usize)
        .collect();
    assert_eq!(
        text.trim_end_matches('│').trim_end(),
        "ab  cd",
        "the tab must expand to exactly 2 columns here"
    );

    let caret_x = caret_column(&buf, EDITOR_TOP_ROW, WIDTH)
        .expect("caret cell must be present on the editor's first row");
    assert_eq!(
        (caret_x - EDITOR_LEFT_COL) as usize,
        expected_visual_col,
        "caret column must agree with wrap's visual_col across a tab"
    );
}

/// Wide-char (CJK, width 2) followed by a tab: the tab's 4-stop math must
/// key off the ACCUMULATED visual column (2, after the wide char), not the
/// char count (1) — and the caret must still agree with `visual_col`.
#[test]
fn wide_char_then_tab_caret_column_agrees_with_wrap_visual_col() {
    let content = "\u{6c49}\tab\n"; // U+6C49 (汉, width 2), tab, "ab"
    let cursor_offset = 4; // byte offset of 'a': 3 bytes of 汉 + 1 byte tab
    let session = app_for(content, cursor_offset, true);
    let app = session.app();

    let view = app.active_doc().view.as_ref().expect("synced view");
    let buffer_point = app.active_doc().buffer.offset_to_line_col(cursor_offset);
    let syntax_point = view.syntax.buffer_to_syntax(buffer_point);
    let wrap_point = view.wrap.syntax_to_wrap(syntax_point);
    let expected_visual_col = view
        .wrap
        .visual_col(content, wrap_point.row, wrap_point.col);
    assert_eq!(
        expected_visual_col, 4,
        "汉 (width 2) then a tab to the next 4-stop must land 'a' at column 4"
    );

    let rows = render::build_rows(app, app.active_doc(), Some(app.active), view);
    let first_row = rows.first().expect("at least one row");
    assert_eq!(
        first_row.first().map(|c| (c.text.as_str(), c.width)),
        Some(("\u{6c49}", 2))
    );
    let tab_cells: Vec<_> = first_row
        .iter()
        .skip(1)
        .take_while(|c| c.text == " " && c.buf_offset == Some(3))
        .collect();
    assert_eq!(
        tab_cells.len(),
        2,
        "the tab (starting at visual col 2) must expand to exactly 2 single-width cells: {first_row:?}"
    );

    let buf = render_to_test_backend(app);
    let caret_x = caret_column(&buf, EDITOR_TOP_ROW, WIDTH)
        .expect("caret cell must be present on the editor's first row");
    assert_eq!((caret_x - EDITOR_LEFT_COL) as usize, expected_visual_col);
}

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

/// Regression for the control-safe cell builder: a non-tab/newline control
/// character (BEL, `\x07`) must never reach `ratatui::buffer::Cell::set_char`
/// either — it gets the Unicode "control picture" placeholder (`U+2407`)
/// instead, at the control char's own `buf_offset`.
#[test]
fn control_char_gets_a_safe_placeholder_glyph() {
    let content = "a\u{7}b\n";
    let session = app_for(content, 0, true);
    let app = session.app();

    let buf = render_to_test_backend(app);
    let text = full_text(&buf, HEIGHT, WIDTH);
    assert!(
        !text.contains('\u{7}'),
        "a raw BEL must never reach the terminal buffer:\n{text:?}"
    );
    assert!(
        text.contains('\u{2407}'),
        "expected the BEL control-picture placeholder (U+2407):\n{text:?}"
    );

    let view = app.active_doc().view.as_ref().expect("synced view");
    let rows = render::build_rows(app, app.active_doc(), Some(app.active), view);
    let placeholder = rows
        .first()
        .and_then(|row| row.iter().find(|c| c.text == "\u{2407}"))
        .expect("placeholder cell present in row 0");
    assert_eq!(
        placeholder.buf_offset,
        Some(1),
        "the BEL is the 2nd byte (offset 1) of \"a\\x07b\""
    );
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

/// Guard for the ONE documented exception to the corpus-agreement test
/// above (`rune_syntax::wrap::grapheme_width`'s doc, `blit`'s narrowed
/// `assert_invariant`, `TODO/TODO.md`): a LONE (single-`char`) cluster that
/// ratatui itself derives width 0 for — a bare combining mark with no base,
/// a stray ZWJ, a lone variation selector, a lone zero-width space — is
/// reserved at width 1 by `control_aware_width`'s clamp, not ratatui's 0,
/// so rune's own caret math always has a cell to land the caret on. This
/// test pins the divergence explicitly (never silently) and proves the
/// whole render path — including `blit`'s own strict-mode assert, live in
/// this `cfg(test)` build — tolerates it without panicking, rather than
/// asserting the two numbers agree (which they deliberately do not here).
#[test]
fn lone_zero_width_cluster_reserves_width_one_though_ratatui_derives_zero() {
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
             if this now fails, the divergence this test guards may no longer exist \
             and the exception can be narrowed or removed"
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
            cell.width, 1,
            "{label}: rune reserves width 1 for a lone zero-width rune (caret reachability)"
        );

        // The end-to-end render must not panic — this exercises `blit`'s
        // own strict-mode `assert_invariant`, narrowed to admit exactly
        // this divergence, in a real cfg(test) build (the strict-invariants
        // gate is armed here).
        let buf = render_to_test_backend(app);
        let text = full_text(&buf, HEIGHT, WIDTH);
        assert!(
            text.contains('a') && text.contains('b'),
            "{label}: the surrounding glyphs must still reach the real backend buffer:\n{text}"
        );
    }
}
