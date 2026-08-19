//! Document-corpus data split out of `palette.rs`: the seed documents,
//! paste/type text palettes, and markdown structural fragments every
//! `cluster_*` strategy in `cluster.rs` draws from.

use crate::driver::DOC_PATH;

/// `(path, content)` seed pairs sessions start from (plan WP7.S3 — a session
/// now opens an arbitrary path, so `DocumentKind` producer selection is
/// reachable, not just markdown). `select` panics on an empty slice (G16) —
/// never let this list go empty. Every entry deliberately excludes any lone
/// `\r` adjacent to a tab inside a nested container (plan Gotcha G1).
pub(in crate::generate) static SEEDS: &[(&str, &str)] = &[
    (DOC_PATH, ""),
    (
        DOC_PATH,
        "Hello there. This is a short prose paragraph with a few sentences in it.\n",
    ),
    (DOC_PATH, "line one\r\nline two\r\nline three\r\n"),
    (DOC_PATH, "\u{feff}hello"),
    (DOC_PATH, "no trailing newline in this document"),
    (
        DOC_PATH,
        "你好世界 🙂 mixed CJK and emoji content 日本語のテスト\n",
    ),
    (
        DOC_PATH,
        "# Title\n\n- item one\n- item two\n\n> a quote\n\n```rust\nfn main() {}\n```\n\n[a link](https://example.com)\n",
    ),
    // WP5.S2: a GFM table seed, so a whole session can start from, edit,
    // and navigate a real rendered table — the only seed that ever gives
    // `render::build_rows`/`row_meta::row_meta` a `TableSegInfo`-bearing
    // segment to walk without relying on `MarkdownWrite` typing one in.
    (
        DOC_PATH,
        "# Doc\n\n| Name | Age |\n| :--- | ---: |\n| Alice | 30 |\n| Bob | 25 |\n\ntail\n",
    ),
    // The only seed whose document opens with a frontmatter delimiter, so a
    // session can edit — and destroy — the YAML code region frontmatter
    // publishes. Every other markdown seed leaves that path unvisited.
    (
        DOC_PATH,
        "---\ntitle: seed\ndraft: true\n---\n\n# Heading\n\nbody text\n",
    ),
    // WP7.S3: non-markdown seeds, opened at a path `rune_ts::lang::resolve`
    // recognises — these exercise `DocumentKind::Code`, whole-document
    // tree-sitter highlighting, and (for `notes.md`) fenced-code highlight
    // together with the markdown producer.
    (
        "/fuzz/main.rs",
        "fn main() {\n    let s = \"escape: \\n and \\t\";\n    // a line comment\n    if true {\n        println!(\"{s}\");\n    }\n}\n",
    ),
    (
        "/fuzz/config.toml",
        "[package]\nname = \"example\"\nversion = \"0.1.0\"\n\n[dependencies]\nserde = \"1.0\"\n",
    ),
    (
        "/fuzz/data.json",
        "{\"name\": \"example\", \"values\": [1, 2, 3], \"nested\": {\"ok\": true}}\n",
    ),
    (
        "/fuzz/script.sh",
        "#!/bin/sh\necho \"hello world\"\nfor f in *.txt; do\n  cat \"$f\"\ndone\n",
    ),
    (
        "/fuzz/mod.tsx",
        "export function Hello() {\n  return <div className=\"a\">Hello, world!</div>;\n}\n",
    ),
    (
        "/fuzz/notes.md",
        "# Notes\n\n```rust\nfn main() {}\n```\n\n```python\ndef f():\n    return 1\n```\n\n```klingon\nQapla'\n```\n\n```\nuntagged fence\n```\n\ntail\n",
    ),
    (
        "/fuzz/opaque.bin",
        "二进制内容 🙂 # not a heading\n\tliteral tab\n你好\n",
    ),
];

/// `Action::Paste`/`Action::ClipboardReply` payloads, verbatim by code
/// point.
/// `Paste`/`ClipboardReply` insert bytes with NO filtering
/// with no filtering, so this is the only place byte-hostile
/// content — CRLF, tab, ZWSP, a real ZWJ family sequence — can reach the
/// buffer and exercise the byte-verbatim edge (G3). Invalid UTF-8 is
/// out of scope: the Rust `Buffer` is a `String` and cannot represent it.
pub(in crate::generate) static PASTE_PALETTE: &[&str] = &[
    "",
    "hello world",
    "你好世界，世界你好",
    "👨‍👩‍👧‍👦 family",
    "❤️ heart FE0F",
    "⚡︎ lightning FE0E",
    "é à ô",
    "مرحبا بالعالم",
    "𝕳𝖊𝖑𝖑𝖔 𝟙𝟚𝟛",
    "aA1! 你好 🙂 mix",
    "line1\r\nline2",
    " \u{200b}\t\u{200b} ",
    "***bold*** _em_ `code`",
    "\n\n\n",
    "\"quoted\" 'text'",
    "12345.6789",
    "a\tb\tc\td",
    "𝓒𝓾𝓻𝓼𝓲𝓿𝓮",
];

/// `Action::Type` payloads: `PASTE_PALETTE` with every `char::is_control()`
/// character except `'\n'` removed, since `Msg::Key(Char)` silently drops
/// control characters (`is_insertable_key_char`, G3).
/// Concretely: drops the CRLF entry and the tab-separated entry, and strips
/// the tab (keeping the ZWSPs, which are format chars, not control chars)
/// from the ZWSP entry. Do not "restore" those — a `Type` cannot deliver
/// them. `pub` (re-exported at `crate::generate::TYPE_PALETTE`) so
/// `tests/generator.rs`'s `type_palette_has_no_undeliverable_control_chars`
/// self-test can inspect every entry.
pub static TYPE_PALETTE: &[&str] = &[
    "",
    "hello world",
    "你好世界，世界你好",
    "👨‍👩‍👧‍👦 family",
    "❤️ heart FE0F",
    "⚡︎ lightning FE0E",
    "é à ô",
    "مرحبا بالعالم",
    "𝕳𝖊𝖑𝖑𝖔 𝟙𝟚𝟛",
    "aA1! 你好 🙂 mix",
    " \u{200b}\u{200b} ",
    "***bold*** _em_ `code`",
    "\n\n\n",
    "\"quoted\" 'text'",
    "12345.6789",
    "𝓒𝓾𝓻𝓼𝓲𝓿𝓮",
];

/// Markdown structural fragments for the `MarkdownWrite` cluster. The
/// three table fragments (WP5.S2) let a session type a table into
/// existence mid-document — a row, a delimiter, and an inline-alignment
/// delimiter — rather than only ever starting from a table seed.
pub(in crate::generate) static MARKDOWN_FRAGMENTS: &[&str] = &[
    "# ",
    "- ",
    "> ",
    "[a](b)",
    "[[wiki]]",
    "**b**",
    "`c`",
    "| a | b |",
    "|---|---|",
    "| :-: |",
];
