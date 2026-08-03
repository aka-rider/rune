//! Split off `highlight_fence.rs` (§1.6): the render-side fence case —
//! a markdown fence's own background must survive the overlay patch while
//! inline markdown ranges inside it still carry their own scopes' styles.
//! Kept apart from the span-production cases because this one asserts on
//! rendered CELLS rather than on stored spans.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod highlight_common;

use highlight_common::{app_for, type_one_char_at_end};
use ratatui::buffer::Buffer as RtBuffer;
use ratatui::style::{Modifier, Style};
use rune_syntax::scope::scope_table;
use rune_tui::app;
use rune_tui::runtime::Effects;
use rune_tui::testgrid;

/// Scans every cell of `buf` (`w` x `h`) for the first place `needle`
/// appears as a run of consecutive single-cell glyphs, and returns that
/// first cell's style — cell-by-cell (never `String::find` on a joined
/// row), the same fence-cell search the overlay tests use: a
/// multi-byte UTF-8 glyph occupies one terminal CELL, so a byte-offset
/// search and a column index silently diverge the moment one precedes the
/// match.
fn find_needle_style(buf: &RtBuffer, w: u16, h: u16, needle: &str) -> Option<Style> {
    let chars: Vec<char> = needle.chars().collect();
    for y in 0..h {
        for x0 in 0..w {
            let matched = chars.iter().enumerate().all(|(k, &nc)| {
                let x = x0 + u16::try_from(k).unwrap_or(u16::MAX);
                buf.cell((x, y))
                    .is_some_and(|cell| cell.symbol() == nc.to_string())
            });
            if matched {
                let cell = buf.cell((x0, y))?;
                return Some(cell.style());
            }
        }
    }
    None
}

/// A ```` ```markdown ```` fence (FOUR backticks, so its own
/// nested three-backtick fence doesn't close it early) gets INLINE markdown
/// highlighting through the comrak reveal-emit reuse path
/// (`runtime::md_fence::markdown_fence_spans`), not flat near-black text.
/// Because `reveal_all` forces every block revealed, the fence's own
/// contents render with their raw markdown markers visible (`# `, `**`,
/// `` ` ``, `[]()`), matching what a real revealed line would show. The
/// heading/bold/code/link ranges must carry their own markdown scopes'
/// styles OVER the fence's `markup.raw.block` background (the overlay's own
/// bg-strip is what lets that background survive the overlay patch), and
/// the nested three-backtick fence's own body must keep that SAME
/// background untouched (its lines resolve to `markup.raw.block` too, so
/// the overlay adds no bg change there either).
#[test]
fn markdown_fence_highlights_inline_markdown_over_the_fence_background() {
    let content = concat!(
        "Intro paragraph.\n",
        "\n",
        "````markdown\n",
        "# Title\n",
        "\n",
        "**bold** `snippet` [linktext](http://example.com)\n",
        "\n",
        "```rust\n",
        "fn main() {}\n",
        "```\n",
        "````\n",
        "\n",
        "Outro.\n",
    );
    let mut app = app_for(content, "/x/notes.md");
    app.sync_view();
    app.doc_mut(app.active)
        .expect("doc")
        .viewport
        .set_size(60, 20);

    let mut effects = Effects::default();
    type_one_char_at_end(&mut app, &mut effects);
    assert_eq!(
        effects.cmds.len(),
        1,
        "expected exactly one scheduled highlight cmd"
    );
    let msg = effects
        .cmds
        .remove(0)
        .run()
        .expect("the highlight cmd always replies");
    let mut effects2 = Effects::default();
    app::update(&mut app, msg, &mut effects2);

    app.sync_view();
    let buf = testgrid::draw(&app, 60, 20);

    let heading_style = app.theme.scope_style(
        scope_table()
            .resolve("markup.heading.1")
            .expect("known scope"),
    );
    let raw_inline_style = app.theme.scope_style(
        scope_table()
            .resolve("markup.raw.inline")
            .expect("known scope"),
    );
    let link_style = app
        .theme
        .scope_style(scope_table().resolve("markup.link").expect("known scope"));
    let fence_bg = app
        .theme
        .scope_style(
            scope_table()
                .resolve("markup.raw.block")
                .expect("known scope"),
        )
        .bg;
    assert!(
        fence_bg.is_some(),
        "the fence background style itself must carry a bg"
    );

    let title = find_needle_style(&buf, 60, 20, "Title").expect("heading text must be on screen");
    assert_eq!(
        title.fg, heading_style.fg,
        "heading text inside the markdown fence must carry the heading fg"
    );
    assert_eq!(
        title.bg, fence_bg,
        "heading text must still sit on the fence's own background"
    );

    let bold = find_needle_style(&buf, 60, 20, "bold").expect("bold text must be on screen");
    assert!(
        bold.add_modifier.contains(Modifier::BOLD),
        "bold text inside the markdown fence must carry the BOLD modifier"
    );
    assert_eq!(
        bold.bg, fence_bg,
        "bold text must still sit on the fence's own background"
    );

    let code =
        find_needle_style(&buf, 60, 20, "snippet").expect("inline-code text must be on screen");
    assert_eq!(
        code.fg, raw_inline_style.fg,
        "inline-code text inside the markdown fence must carry the raw.inline fg"
    );

    let link = find_needle_style(&buf, 60, 20, "linktext").expect("link text must be on screen");
    assert_eq!(
        link.fg, link_style.fg,
        "link text inside the markdown fence must carry the link fg"
    );
    assert!(
        link.add_modifier.contains(Modifier::UNDERLINED),
        "link text inside the markdown fence must carry the UNDERLINED modifier"
    );

    let inner_fence_body =
        find_needle_style(&buf, 60, 20, "fn main").expect("nested fence body must be on screen");
    assert_eq!(
        inner_fence_body.bg, fence_bg,
        "the nested three-backtick fence's own body must keep the outer fence background"
    );
}
