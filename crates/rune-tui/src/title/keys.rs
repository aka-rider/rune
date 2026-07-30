//! Keystroke handling for the focused title (`Pane::Title`, stage 3 of
//! `dispatch::handle_key`) and the blur commit chokepoint every focus
//! transition away from the title runs through. A submodule of `title`
//! (Rust lets a `title.rs` with a `title/` subdirectory — `keymap.rs` +
//! `keymap/` is the in-repo precedent), so it reads `TitleField`'s private
//! fields directly rather than through the public accessors `render::
//! title` (a sibling module) is limited to.
//!
//! `handle_key` resolves in three tiers, checked in order:
//! 1. **Pane keys** — Enter/Down commit by moving focus (the blur below
//!    does the actual work); Escape reverts FIRST, then moves focus, so it
//!    stays an unconditional exit even when a commit would be refused
//!    (gotcha 8). Both match ANY modifiers, exactly as before `TitleField`
//!    grew a real editor: ⌘Enter and ⇧Down commit too.
//! 2. **The extension gate** — a bare `Right` sitting exactly at the
//!    stem/extension split unlocks the extension for the rest of this
//!    focus session, without moving the cursor (assumption A3: only a
//!    plain, unmodified `Right`). Everywhere else `Right` falls through to
//!    tier 3 like any other motion.
//! 3. **The editor table** — `crate::keymap::editor_bindings::
//!    EDITOR_BINDINGS`, windowed to `TitleField::window()`, with a
//!    filtered printable-character fallback for chords the table doesn't
//!    resolve. No new binding rows (plan DON'Ts) — this reuses the exact
//!    table the document editor does.
//!
//! Every path returns [`KeyOutcome::Consumed`]: stage 3 for `Pane::Title`
//! never lets a keystroke fall through to the buffer, which is what the
//! fuzzer's `PANE-NO-BLEED` invariant asserts.

use crate::app::App;
use crate::keymap::{self, KeyCode, KeyInput, KeyOutcome, Mods};
use crate::pane::Pane;
use crate::rename;
use crate::runtime::Effects;

use super::{INVALID_NAME_CHARS, TitleField, ext_split};

pub fn handle_key(app: &mut App, key: KeyInput, effects: &mut Effects) -> KeyOutcome {
    match key.code {
        // Enter/Down do nothing but move focus — the blur commits (decision
        // 4: DRY, one commit chokepoint).
        KeyCode::Enter | KeyCode::Down => {
            app.set_focus(Pane::Editor, effects);
            return KeyOutcome::Consumed;
        }
        // Escape reverts FIRST, then releases focus — reversed, it would
        // commit the abandoned name, and this ordering is what keeps
        // Escape an unconditional exit even when `on_blur` would otherwise
        // veto (gotcha 8).
        KeyCode::Escape => {
            app.title.revert();
            app.set_focus(Pane::Editor, effects);
            return KeyOutcome::Consumed;
        }
        _ => {}
    }

    if key.code == KeyCode::Right && key.mods == Mods::NONE && try_unlock_extension(&mut app.title)
    {
        return KeyOutcome::Consumed;
    }

    let window = app.title.window();
    if let Some(cmd) = keymap::resolve_in(keymap::editor_bindings::EDITOR_BINDINGS, key) {
        let _ = app.title.field_mut().apply(cmd, window);
    } else if let KeyCode::Char(ch) = key.code
        && !key.mods.ctrl
        && !key.mods.alt
        && !key.mods.sup
        && !ch.is_control()
        && !INVALID_NAME_CHARS.contains(&ch)
    {
        let _ = app.title.field_mut().insert(&ch.to_string(), window);
    }
    KeyOutcome::Consumed
}

/// The Right-at-end-of-stem gesture. Unlocks the gate without moving the
/// cursor; a no-op unless the gate is currently locked, the cursor sits
/// exactly at the split, AND there is an extension to unlock at all
/// (`split < len`) — an empty stem already seeds unlocked (decision 9), so
/// this never has anything to do there, and a second `Right` press with
/// the gate already open falls straight through to ordinary motion.
fn try_unlock_extension(title: &mut TitleField) -> bool {
    let split = ext_split(title.field.text());
    let len = title.field.len();
    let at_split = title.field.cursor().position == split;
    if !title.ext_unlocked && at_split && split < len {
        title.ext_unlocked = true;
        true
    } else {
        false
    }
}

/// The single commit chokepoint (decision 4/8): whether the title may
/// release focus. Called from exactly one place, `App::set_focus`,
/// whenever the title currently holds focus and something else wants it —
/// every site that changes the active document, every chrome-focus
/// command, and the hoisted gate in `pane::handle_global_command` all
/// reach this indirectly through `set_focus`/`blur_title`, never directly.
///
/// A commit with an unchanged name is `Accepted` outright — a plain
/// refocus, never a rename of a file onto its own path. Otherwise
/// `rename::begin` decides every refusal (read-only, a save in flight, an
/// invalid name, a rename already in progress) and owns the whole workflow
/// from here. `committed` is deliberately NOT advanced on the way out: it
/// is the name the file actually has, and it moves only once a rename has
/// really landed (`rename::bind_to` reseeds the field). Advancing it
/// optimistically here would make `Esc` revert to a name no file has.
pub fn on_blur(app: &mut App, effects: &mut Effects) -> rename::Commit {
    if app.title.text() == app.title.committed() {
        return rename::Commit::Accepted;
    }
    rename::begin(app, effects)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn field_for(name: &str) -> TitleField {
        let mut field = TitleField::default();
        field.seed(name);
        field
    }

    #[test]
    fn right_at_the_split_unlocks_without_moving_the_cursor() {
        let mut field = field_for("lessrc.md");
        assert!(!field.ext_unlocked());
        let cursor_before = field.field().cursor().position;
        assert!(try_unlock_extension(&mut field));
        assert!(field.ext_unlocked());
        assert_eq!(field.field().cursor().position, cursor_before);
    }

    #[test]
    fn right_away_from_the_split_does_not_unlock() {
        let mut field = field_for("lessrc.md");
        field.field_mut().set_cursor(0, 0);
        assert!(!try_unlock_extension(&mut field));
        assert!(!field.ext_unlocked());
    }

    #[test]
    fn an_already_unlocked_gate_is_a_no_op() {
        let mut field = field_for(".md");
        assert!(field.ext_unlocked());
        assert!(!try_unlock_extension(&mut field));
    }

    #[test]
    fn on_blur_accepts_an_unchanged_name_without_starting_a_rename() {
        let mut app = crate::app::App::new(
            rune_core::buffer::Buffer::new("body"),
            Some(std::path::PathBuf::from("/root/a.md")),
            std::sync::Arc::new(rune_vfs::Mem::new()),
            None,
        );
        app.title.seed("a.md");
        let mut effects = Effects::default();
        assert_eq!(on_blur(&mut app, &mut effects), rename::Commit::Accepted);
        assert!(effects.cmds.is_empty());
    }
}
