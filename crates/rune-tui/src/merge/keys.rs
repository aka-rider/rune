//! The resolver's own key surface (plan WP4.S1): a named binding table so
//! `ValidateNoPhysicalKeyCollisions`-style guard tests can see every merge
//! chord (§3.1), and the dispatch intercept that owns EVERY key while the
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
        keys: &[KeyPattern::new(KeyCode::Char('['), Mods::NONE)],
        cmd: MergeCommand::PrevConflict,
        help: "prev conflict",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char(']'), Mods::NONE)],
        cmd: MergeCommand::NextConflict,
        help: "next conflict",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('o'), Mods::NONE)],
        cmd: MergeCommand::KeepOurs,
        help: "keep editor's side",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('O'), SHIFT)],
        cmd: MergeCommand::KeepOurs,
        help: "keep editor's side",
        when: "",
        alias: true,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('t'), Mods::NONE)],
        cmd: MergeCommand::KeepTheirs,
        help: "keep disk's side",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('T'), SHIFT)],
        cmd: MergeCommand::KeepTheirs,
        help: "keep disk's side",
        when: "",
        alias: true,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('b'), Mods::NONE)],
        cmd: MergeCommand::KeepBoth,
        help: "keep both (markers stay)",
        when: "",
        alias: false,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Char('B'), SHIFT)],
        cmd: MergeCommand::KeepBoth,
        help: "keep both (markers stay)",
        when: "",
        alias: true,
    },
    Binding {
        keys: &[KeyPattern::new(KeyCode::Escape, Mods::NONE)],
        cmd: MergeCommand::Exit,
        help: "close merge",
        when: "",
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

    // Reading a long working form is normal mid-merge; the viewport keys
    // keep working (any modifier accepted, mirroring `reading_nav`'s
    // shift-scrolls-the-same convention).
    match key.code {
        KeyCode::Up => nav_scroll::scroll_lines(app.active_doc_mut(), -1),
        KeyCode::Down => nav_scroll::scroll_lines(app.active_doc_mut(), 1),
        KeyCode::PageUp => {
            let step = nav_scroll::page_step(app.active_doc());
            nav_scroll::scroll_lines(app.active_doc_mut(), -step);
        }
        KeyCode::PageDown => {
            let step = nav_scroll::page_step(app.active_doc());
            nav_scroll::scroll_lines(app.active_doc_mut(), step);
        }
        KeyCode::Home => nav_scroll::scroll_to_document_top(app.active_doc_mut()),
        KeyCode::End => nav_scroll::scroll_to_document_bottom(app.active_doc_mut()),
        _ => messages::warn(app, MERGE_KEY_HINT),
    }
    true
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
            for pattern in binding.keys {
                let key = match pattern.key {
                    crate::binding::KeyMatch::Code(code) => KeyInput {
                        code,
                        mods: pattern.mods,
                    },
                    crate::binding::KeyMatch::Printable => continue,
                };
                let claimants: Vec<&'static str> = GLOBAL_BINDINGS
                    .iter()
                    .filter(|b| b.keys.iter().any(|k| k.matches(key)))
                    .map(|b| b.help)
                    .collect();
                assert!(
                    claimants.is_empty(),
                    "GLOBAL_BINDINGS would shadow merge key {key:?}: {claimants:?}"
                );
            }
        }
    }
}
