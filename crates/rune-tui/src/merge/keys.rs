//! The resolver's own key surface (plan WP4.S1): a named binding table so
//! `ValidateNoPhysicalKeyCollisions`-style guard tests can see every merge
//! chord, and the dispatch intercept that owns EVERY key while the
//! resolver is active — the working form is a live buffer, and any key that
//! slipped through to the printable-insert fallthrough would type into it
//! and desync the block spans. Consuming with feedback, never silently, is
//! the House Rule this module exists to uphold.

use crate::app::App;
use crate::binding::{Binding, KeyPattern, resolve_in};
use crate::commands::nav_scroll;
use crate::keymap::{KeyCode, KeyInput, Mods};
use crate::messages;

use super::state::MergeState;

/// What the swallow-status advertises for any key the resolver does not
/// bind; also the fallback wording tests assert against.
const MERGE_KEY_HINT: &str = "merge: [O]urs [T]heirs [B]oth · [ ] navigate · Esc close";

const SHIFT: Mods = Mods {
    shift: true,
    alt: false,
    ctrl: false,
    sup: false,
};

/// The resolver's commands — resolved ONLY by `intercept` below, never by
/// the global or editor tables, so these bare letters can exist without
/// shadowing text input anywhere else (the invariant the
/// `merge_keys_are_not_bound_in_the_global_table` guard test pins).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MergeCommand {
    PrevConflict,
    NextConflict,
    KeepOurs,
    KeepTheirs,
    KeepBoth,
    Exit,
}

pub const MERGE_BINDINGS: &[Binding<MergeCommand>] = &[
    Binding {
        key: KeyPattern::new(KeyCode::Char('['), Mods::NONE),
        cmd: MergeCommand::PrevConflict,
        help: "prev conflict",
        alias: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Char(']'), Mods::NONE),
        cmd: MergeCommand::NextConflict,
        help: "next conflict",
        alias: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Char('o'), Mods::NONE),
        cmd: MergeCommand::KeepOurs,
        help: "keep editor's side",
        alias: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Char('O'), SHIFT),
        cmd: MergeCommand::KeepOurs,
        help: "keep editor's side",
        alias: true,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Char('t'), Mods::NONE),
        cmd: MergeCommand::KeepTheirs,
        help: "keep disk's side",
        alias: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Char('T'), SHIFT),
        cmd: MergeCommand::KeepTheirs,
        help: "keep disk's side",
        alias: true,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Char('b'), Mods::NONE),
        cmd: MergeCommand::KeepBoth,
        help: "keep both (markers stay)",
        alias: false,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Char('B'), SHIFT),
        cmd: MergeCommand::KeepBoth,
        help: "keep both (markers stay)",
        alias: true,
    },
    Binding {
        key: KeyPattern::new(KeyCode::Escape, Mods::NONE),
        cmd: MergeCommand::Exit,
        help: "close merge",
        alias: false,
    },
];

/// The resolver's whole-keyboard capture (plan WP4.S1, Gotchas `[B1]`):
/// MUST be the first thing `dispatch::handle_editor_key` runs — before the
/// hardcoded Enter/Escape fast paths and the printable-insert fallthrough —
/// or `o`/`t`/`b` type into the working form and bare Esc collapses the
/// selection instead of closing the merge. Returns `false` (untouched key)
/// unless the resolver is active ON the active document; after that, every
/// key is consumed here: a merge command, a viewport scroll, or the
/// swallow-status.
pub(crate) fn intercept(app: &mut App, key: KeyInput) -> bool {
    let MergeState::Active { doc, .. } = &app.merge else {
        return false;
    };
    if *doc != app.active {
        return false;
    }

    if let Some(cmd) = resolve_in(MERGE_BINDINGS, key) {
        match cmd {
            MergeCommand::PrevConflict => super::resolve::nav(app, -1),
            MergeCommand::NextConflict => super::resolve::nav(app, 1),
            MergeCommand::KeepOurs => super::resolve::accept(app, super::resolve::Choice::Ours),
            MergeCommand::KeepTheirs => {
                super::resolve::accept(app, super::resolve::Choice::Theirs);
            }
            MergeCommand::KeepBoth => super::resolve::accept(app, super::resolve::Choice::Both),
            MergeCommand::Exit => super::exit_in_place(app),
        }
        return true;
    }

    // Reading a long working form is normal mid-merge, so the viewport keys
    // keep working — but only the bare/shift arrows `viewport_scroll`
    // recognises; every other chord (an ordinary editor command with no
    // meaning here) falls through to the swallow-status below.
    match viewport_scroll(key) {
        Some(ScrollKey::LineUp) => nav_scroll::scroll_lines(app.active_doc_mut(), -1),
        Some(ScrollKey::LineDown) => nav_scroll::scroll_lines(app.active_doc_mut(), 1),
        Some(ScrollKey::PageUp) => {
            let step = nav_scroll::page_step(app.active_doc());
            nav_scroll::scroll_lines(app.active_doc_mut(), -step);
        }
        Some(ScrollKey::PageDown) => {
            let step = nav_scroll::page_step(app.active_doc());
            nav_scroll::scroll_lines(app.active_doc_mut(), step);
        }
        Some(ScrollKey::Top) => nav_scroll::scroll_to_document_top(app.active_doc_mut()),
        Some(ScrollKey::Bottom) => nav_scroll::scroll_to_document_bottom(app.active_doc_mut()),
        None => messages::warn(app, MERGE_KEY_HINT),
    }
    true
}

/// One of the viewport-scroll requests the merge resolver honours while it
/// owns the keyboard — the working form is read-only from here, so these are
/// the whole navigation vocabulary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollKey {
    LineUp,
    LineDown,
    PageUp,
    PageDown,
    Top,
    Bottom,
}

/// The sole source of truth for which keys the resolver treats as "read the
/// working form" rather than an editor command it must refuse: a bare arrow/
/// Home/End/PageUp/PageDown, or the same key held with Shift — Shift scrolls
/// exactly like its bare key because keyboard selection has no working-form
/// buffer to select in. Every other chord (alt, ctrl, or sup held) is an
/// ordinary editor command with no meaning here, and must be refused out
/// loud rather than silently re-keyed into a scroll.
pub fn viewport_scroll(key: KeyInput) -> Option<ScrollKey> {
    if key.mods != Mods::NONE && key.mods != SHIFT {
        return None;
    }
    match key.code {
        KeyCode::Up => Some(ScrollKey::LineUp),
        KeyCode::Down => Some(ScrollKey::LineDown),
        KeyCode::PageUp => Some(ScrollKey::PageUp),
        KeyCode::PageDown => Some(ScrollKey::PageDown),
        KeyCode::Home => Some(ScrollKey::Top),
        KeyCode::End => Some(ScrollKey::Bottom),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::global::GLOBAL_BINDINGS;

    /// Stage 2 (`GLOBAL_BINDINGS`) resolves before the editor pane — and
    /// therefore before `intercept` — ever sees a key, so a global row
    /// matching any merge chord would shadow the resolver silently. Same
    /// dispatch-time `KeyPattern::matches` predicate as the `^p`/`⌘p`
    /// guard test, for the same reason: structural equality can't see a
    /// `Printable` wildcard steal.
    #[test]
    fn merge_keys_are_not_bound_in_the_global_table() {
        for binding in MERGE_BINDINGS {
            let pattern = binding.key;
            let key = match pattern.key {
                crate::binding::KeyMatch::Code(code) => KeyInput {
                    code,
                    mods: pattern.mods,
                },
                crate::binding::KeyMatch::Printable => continue,
            };
            let claimants: Vec<&'static str> = GLOBAL_BINDINGS
                .iter()
                .filter(|b| b.key.matches(key))
                .map(|b| b.help)
                .collect();
            assert!(
                claimants.is_empty(),
                "GLOBAL_BINDINGS would shadow merge key {key:?}: {claimants:?}"
            );
        }
    }

    const ALT: Mods = Mods {
        shift: false,
        alt: true,
        ctrl: false,
        sup: false,
    };
    const CTRL: Mods = Mods {
        shift: false,
        alt: false,
        ctrl: true,
        sup: false,
    };
    const SUP: Mods = Mods {
        shift: false,
        alt: false,
        ctrl: false,
        sup: true,
    };
    const ALT_SUP: Mods = Mods {
        shift: false,
        alt: true,
        ctrl: false,
        sup: true,
    };
    const SHIFT_ALT: Mods = Mods {
        shift: true,
        alt: true,
        ctrl: false,
        sup: false,
    };

    /// A bare or shift-only arrow/Home/End/PageUp/PageDown is the resolver's
    /// whole "read the working form" vocabulary; any other chord on those
    /// same keys is an ordinary editor command the resolver must refuse
    /// rather than silently re-key into a scroll.
    #[test]
    fn only_bare_and_shift_arrows_are_viewport_scrolls() {
        let cases = [
            (KeyCode::Up, Mods::NONE, Some(ScrollKey::LineUp)),
            (KeyCode::Up, SHIFT, Some(ScrollKey::LineUp)),
            (KeyCode::Down, Mods::NONE, Some(ScrollKey::LineDown)),
            (KeyCode::Down, SHIFT, Some(ScrollKey::LineDown)),
            (KeyCode::PageUp, Mods::NONE, Some(ScrollKey::PageUp)),
            (KeyCode::PageUp, SHIFT, Some(ScrollKey::PageUp)),
            (KeyCode::PageDown, Mods::NONE, Some(ScrollKey::PageDown)),
            (KeyCode::PageDown, SHIFT, Some(ScrollKey::PageDown)),
            (KeyCode::Home, Mods::NONE, Some(ScrollKey::Top)),
            (KeyCode::Home, SHIFT, Some(ScrollKey::Top)),
            (KeyCode::End, Mods::NONE, Some(ScrollKey::Bottom)),
            (KeyCode::End, SHIFT, Some(ScrollKey::Bottom)),
            // AddCursorAbove/AddCursorBelow.
            (KeyCode::Up, ALT_SUP, None),
            (KeyCode::Down, ALT_SUP, None),
            // CloneLineUp.
            (KeyCode::Up, SHIFT_ALT, None),
            // A bare `sup`-only chord.
            (KeyCode::Up, SUP, None),
            (KeyCode::Up, ALT, None),
            (KeyCode::Up, CTRL, None),
        ];
        for (code, mods, want) in cases {
            let got = viewport_scroll(KeyInput { code, mods });
            assert_eq!(got, want, "code={code:?} mods={mods:?}");
        }
    }
}
