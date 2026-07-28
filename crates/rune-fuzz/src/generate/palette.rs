//! Static session-generation data, split out of `generate` (§1.6 budget):
//! the four string palettes and the fixed `KeyInput`s/`KeyInput` slices
//! every `cluster_*` strategy in `cluster.rs` draws from.

use rune_tui::keymap::{KeyCode, KeyInput, Mods};

/// Seed documents sessions start from. `select` panics on an empty slice
/// (G16) — never let this list go empty. Deliberately excludes any lone `\r`
/// adjacent to a tab inside a nested container (G1: known-open comrak
/// strict-invariants panic, `rune-md/TODO.md`).
pub(super) static CONTENT_SEEDS: &[&str] = &[
    "",
    "Hello there. This is a short prose paragraph with a few sentences in it.\n",
    "line one\r\nline two\r\nline three\r\n",
    "\u{feff}hello",
    "no trailing newline in this document",
    "你好世界 🙂 mixed CJK and emoji content 日本語のテスト\n",
    "# Title\n\n- item one\n- item two\n\n> a quote\n\n```rust\nfn main() {}\n```\n\n[a link](https://example.com)\n",
    // WP5.S2: a GFM table seed, so a whole session can start from, edit,
    // and navigate a real rendered table — the only seed that ever gives
    // `render::build_rows`/`row_meta::row_meta` a `TableSegInfo`-bearing
    // segment to walk without relying on `MarkdownWrite` typing one in.
    "# Doc\n\n| Name | Age |\n| :--- | ---: |\n| Alice | 30 |\n| Bob | 25 |\n\ntail\n",
];

/// `Action::Paste`/`Action::ClipboardReply` payloads: Go's 16 entries,
/// verbatim by code point.
/// `Paste`/`ClipboardReply` insert bytes with NO filtering
/// with no filtering, so this is the only place byte-hostile
/// content — CRLF, tab, ZWSP, a real ZWJ family sequence — can reach the
/// buffer and exercise the §1.4.5 byte-verbatim edge (G3). Invalid UTF-8 is
/// out of scope: the Rust `Buffer` is a `String` and cannot represent it.
pub(super) static PASTE_PALETTE: &[&str] = &[
    "",
    "hello world",
    "你好世界，世界你好",
    "👨‍👩‍👧‍👦 family",
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
pub(super) static MARKDOWN_FRAGMENTS: &[&str] = &[
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

/// The eight motions the `Navigate` cluster draws from, plus alt+Left/Right.
pub(super) static NAV_KEYS: &[KeyInput] = &[
    KeyInput {
        code: KeyCode::Left,
        mods: Mods::NONE,
    },
    KeyInput {
        code: KeyCode::Right,
        mods: Mods::NONE,
    },
    KeyInput {
        code: KeyCode::Up,
        mods: Mods::NONE,
    },
    KeyInput {
        code: KeyCode::Down,
        mods: Mods::NONE,
    },
    KeyInput {
        code: KeyCode::Home,
        mods: Mods::NONE,
    },
    KeyInput {
        code: KeyCode::End,
        mods: Mods::NONE,
    },
    KeyInput {
        code: KeyCode::PageUp,
        mods: Mods::NONE,
    },
    KeyInput {
        code: KeyCode::PageDown,
        mods: Mods::NONE,
    },
    KeyInput {
        code: KeyCode::Left,
        mods: Mods {
            shift: false,
            alt: true,
            ctrl: false,
            sup: false,
        },
    },
    KeyInput {
        code: KeyCode::Right,
        mods: Mods {
            shift: false,
            alt: true,
            ctrl: false,
            sup: false,
        },
    },
];

/// The same eight motions, shift-modified, for the `Selection` cluster.
pub(super) static SELECT_MOTION_KEYS: &[KeyInput] = &[
    KeyInput {
        code: KeyCode::Left,
        mods: Mods {
            shift: true,
            alt: false,
            ctrl: false,
            sup: false,
        },
    },
    KeyInput {
        code: KeyCode::Right,
        mods: Mods {
            shift: true,
            alt: false,
            ctrl: false,
            sup: false,
        },
    },
    KeyInput {
        code: KeyCode::Up,
        mods: Mods {
            shift: true,
            alt: false,
            ctrl: false,
            sup: false,
        },
    },
    KeyInput {
        code: KeyCode::Down,
        mods: Mods {
            shift: true,
            alt: false,
            ctrl: false,
            sup: false,
        },
    },
    KeyInput {
        code: KeyCode::Home,
        mods: Mods {
            shift: true,
            alt: false,
            ctrl: false,
            sup: false,
        },
    },
    KeyInput {
        code: KeyCode::End,
        mods: Mods {
            shift: true,
            alt: false,
            ctrl: false,
            sup: false,
        },
    },
    KeyInput {
        code: KeyCode::PageUp,
        mods: Mods {
            shift: true,
            alt: false,
            ctrl: false,
            sup: false,
        },
    },
    KeyInput {
        code: KeyCode::PageDown,
        mods: Mods {
            shift: true,
            alt: false,
            ctrl: false,
            sup: false,
        },
    },
];

/// The `Delete` cluster's four keys.
pub(super) static DELETE_KEYS: &[KeyInput] = &[
    KeyInput {
        code: KeyCode::Backspace,
        mods: Mods::NONE,
    },
    KeyInput {
        code: KeyCode::Delete,
        mods: Mods::NONE,
    },
    KeyInput {
        code: KeyCode::Tab,
        mods: Mods::NONE,
    },
    KeyInput {
        code: KeyCode::BackTab,
        mods: Mods::NONE,
    },
];

pub(super) const SELECT_ALL_KEY: KeyInput = KeyInput {
    code: KeyCode::Char('a'),
    mods: Mods {
        shift: false,
        alt: false,
        ctrl: false,
        sup: true,
    },
};
pub(super) const UNDO_KEY: KeyInput = KeyInput {
    code: KeyCode::Char('z'),
    mods: Mods {
        shift: false,
        alt: false,
        ctrl: false,
        sup: true,
    },
};
pub(super) const REDO_KEY: KeyInput = KeyInput {
    code: KeyCode::Char('z'),
    mods: Mods {
        shift: true,
        alt: false,
        ctrl: false,
        sup: true,
    },
};
pub(super) const ENTER_KEY: KeyInput = KeyInput {
    code: KeyCode::Enter,
    mods: Mods::NONE,
};
pub(super) const SAVE_KEY: KeyInput = KeyInput {
    code: KeyCode::Char('s'),
    mods: Mods {
        shift: false,
        alt: false,
        ctrl: false,
        sup: true,
    },
};
pub(super) const COPY_KEY: KeyInput = KeyInput {
    code: KeyCode::Char('c'),
    mods: Mods {
        shift: false,
        alt: false,
        ctrl: false,
        sup: true,
    },
};
pub(super) const CUT_KEY: KeyInput = KeyInput {
    code: KeyCode::Char('x'),
    mods: Mods {
        shift: false,
        alt: false,
        ctrl: false,
        sup: true,
    },
};
pub(super) const PASTE_KEY: KeyInput = KeyInput {
    code: KeyCode::Char('v'),
    mods: Mods {
        shift: false,
        alt: false,
        ctrl: false,
        sup: true,
    },
};
pub(super) const CTRL_C_KEY: KeyInput = KeyInput {
    code: KeyCode::Char('c'),
    mods: Mods {
        shift: false,
        alt: false,
        ctrl: true,
        sup: false,
    },
};

/// `^r` (`GlobalCommand::FocusTitle`) — reaching `Pane::Title` is what
/// extends `PANE-NO-BLEED` to cover "typing a filename never touches a
/// buffer byte". Every subsequent generated character then lands in the
/// title field instead of the document, which is precisely the property
/// worth fuzzing.
pub(super) const CTRL_R_KEY: KeyInput = KeyInput {
    code: KeyCode::Char('r'),
    mods: Mods {
        shift: false,
        alt: false,
        ctrl: true,
        sup: false,
    },
};
