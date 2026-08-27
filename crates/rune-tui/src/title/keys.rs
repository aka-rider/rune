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

use std::ops::Range;

use crate::app::App;
use crate::clipboard::pbpaste_cmd;
use crate::commands::clipboard::write_to_clipboard_or_report;
use crate::document::DocumentId;
use crate::focus::{self, FocusTarget};
use crate::keymap::{self, Command, KeyCode, KeyInput, KeyOutcome, Mods};
use crate::pane::Pane;
use crate::rename;
use crate::runtime::{Effects, PasteTarget};

use super::{TitleField, ext_split};

pub fn handle_key(app: &mut App, key: KeyInput, effects: &mut Effects) -> KeyOutcome {
    match key.code {
        // Enter/Down do nothing but move focus — the blur commits (decision
        // 4: DRY, one commit chokepoint).
        KeyCode::Enter | KeyCode::Down => {
            app.set_focus_pane(Pane::Editor, effects);
            return KeyOutcome::Consumed;
        }
        // Escape reverts FIRST, then releases focus — reversed, it would
        // commit the abandoned name, and this ordering is what keeps
        // Escape an unconditional exit even when `on_blur` would otherwise
        // veto (gotcha 8).
        KeyCode::Escape => {
            app.title.revert();
            app.set_focus_pane(Pane::Editor, effects);
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
        match cmd {
            // Copy and cut both operate on the selection, or — with no
            // selection — on `window` (assumption A2), so the two ranges
            // can never disagree: taking the whole name for copy while cut
            // could only delete the window would leave the extension
            // behind and paste back a doubled one.
            Command::Copy => {
                let text = copy_range_text(app);
                write_to_clipboard_or_report(app, &text, effects);
            }
            Command::Cut => {
                // Resolved ONCE and reused: copying and deleting must cover
                // the identical range (assumption A2), and two calls would
                // leave that true only by accident of nothing mutating the
                // cursor in between.
                let range = selection_or_window(app);
                let text = range_text(app, range.clone());
                write_to_clipboard_or_report(app, &text, effects);
                let _ = app.title.field_mut().delete_range(range);
            }
            Command::Paste => effects
                .cmds
                .push(pbpaste_cmd(PasteTarget::Title(app.active))),
            _ => {
                let _ = app.title.field_mut().apply(cmd, window);
            }
        }
    } else if let KeyCode::Char(ch) = key.code
        && !key.mods.ctrl
        && !key.mods.alt
        && !key.mods.sup
        && crate::title::is_name_char(ch)
    {
        let _ = app.title.field_mut().insert(&ch.to_string(), window);
    }
    KeyOutcome::Consumed
}

/// The range copy/cut both act on: the live selection when there is one,
/// else the currently-editable `window` — never the whole name, so a
/// locked gate never lets ⌘C/⌘X reach the fenced-off extension (assumption
/// A2).
fn selection_or_window(app: &App) -> Range<usize> {
    let cursor = app.title.field().cursor();
    if cursor.has_selection() {
        let (start, end) = cursor.selection_range();
        start.get()..end.get()
    } else {
        app.title.window()
    }
}

/// The field's text over `range`, empty when `range` doesn't land on
/// `char` boundaries.
fn range_text(app: &App, range: Range<usize>) -> String {
    app.title
        .field()
        .text()
        .get(range)
        .unwrap_or("")
        .to_string()
}

fn copy_range_text(app: &App) -> String {
    range_text(app, selection_or_window(app))
}

/// Handles a paste routed to the title — a title-focused `Msg::Paste`
/// (bracketed paste) or a `Msg::ClipboardRead` carrying `PasteTarget::
/// Title` (`dispatch::update_inner`). No-ops unless the title STILL has
/// focus: `pbpaste` runs on its own thread and can take a while, and a
/// late reply must not write into a field the user has since left.
/// Sanitizes through [`sanitize_name_input`] — a pasted file name is
/// filtered exactly like a typed one, first line only, so a multi-line or
/// control-byte-laden clipboard payload can never leave the field holding
/// something `is_valid_name` would refuse anyway. A sanitized result that
/// comes out empty is a no-op rather than an insert of nothing.
pub fn paste(app: &mut App, doc: DocumentId, text: &str) {
    if focus::target(app) != FocusTarget::Title || app.active != doc {
        return;
    }
    let sanitized = sanitize_name_input(text);
    if sanitized.is_empty() {
        return;
    }
    let window = app.title.window();
    let _ = app.title.field_mut().insert(&sanitized, window);
}

/// First line only, everything [`crate::title::is_name_char`] rejects dropped —
/// the same restrictions ordinary character-at-a-time typing enforces in
/// `handle_key`'s tier 3, applied at once to a pasted string instead of
/// one `char` at a time.
fn sanitize_name_input(text: &str) -> String {
    text.lines()
        .next()
        .unwrap_or("")
        .chars()
        .filter(|ch| crate::title::is_name_char(*ch))
        .collect()
}

/// The Right-at-end-of-stem gesture. Unlocks the gate without moving the
/// cursor; a no-op unless the gate is currently locked, the cursor sits
/// exactly at the split, AND there is an extension to unlock at all
/// (`split < len`) — an empty stem already seeds unlocked (decision 9), so
/// this never has anything to do there, and a second `Right` press with
/// the gate already open falls straight through to ordinary motion.
fn try_unlock_extension(title: &mut TitleField) -> bool {
    if can_unlock_extension(title) {
        title.ext_unlocked = true;
        return true;
    }
    false
}

/// Whether Right would actually unlock the extension right now: the gate is
/// still locked, there IS an extension to unlock, and the cursor sits
/// exactly at the split. The footer advertises the gesture through this same
/// predicate, so the hint can never promise a keypress that does nothing.
pub(crate) fn can_unlock_extension(title: &TitleField) -> bool {
    let split = ext_split(title.field.text());
    !title.ext_unlocked && split < title.field.len() && title.field.cursor().position.get() == split
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
