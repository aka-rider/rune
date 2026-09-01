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

// Every path returns `KeyOutcome::Consumed`: a keystroke reaching this
// stage must never fall through to the buffer, which is what the fuzzer's
// `PANE-NO-BLEED` invariant asserts.
pub fn handle_key(app: &mut App, key: KeyInput, effects: &mut Effects) -> KeyOutcome {
    match key.code {
        // Enter/Down do nothing but move focus — `on_blur` below does the
        // actual commit.
        KeyCode::Enter | KeyCode::Down => {
            app.set_focus_pane(Pane::Editor, effects);
            return KeyOutcome::Consumed;
        }
        // Reverts FIRST, then releases focus: reversed, it would commit
        // the abandoned name, and this ordering is what keeps Escape an
        // unconditional exit even when `on_blur` would otherwise veto it.
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
            // selection — on `window`, so the two ranges can never
            // disagree: taking the whole name for copy while cut could
            // only delete the window would leave the extension behind and
            // paste back a doubled one.
            Command::Copy => {
                let text = copy_range_text(app);
                write_to_clipboard_or_report(app, &text, effects);
            }
            Command::Cut => {
                // Resolved once and reused, so copying and deleting cover
                // the identical range even if something between the two
                // calls could otherwise move the cursor.
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
/// locked gate never lets ⌘C/⌘X reach the fenced-off extension.
fn selection_or_window(app: &App) -> Range<usize> {
    let cursor = app.title.field().cursor();
    if cursor.has_selection() {
        let (start, end) = cursor.selection_range();
        start.get()..end.get()
    } else {
        app.title.window()
    }
}

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

/// No-ops unless the title STILL has focus: `pbpaste` runs on its own
/// thread and can take a while, and a late reply must not write into a
/// field the user has since left.
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

/// First line only, everything [`crate::title::is_name_char`] rejects
/// dropped — the same restrictions ordinary typing enforces in
/// `handle_key`, applied at once to a pasted string instead of one `char`
/// at a time.
fn sanitize_name_input(text: &str) -> String {
    text.lines()
        .next()
        .unwrap_or("")
        .chars()
        .filter(|ch| crate::title::is_name_char(*ch))
        .collect()
}

fn try_unlock_extension(title: &mut TitleField) -> bool {
    if can_unlock_extension(title) {
        title.ext_unlocked = true;
        return true;
    }
    false
}

/// The footer advertises the unlock gesture through this same predicate,
/// so the hint can never promise a keypress that does nothing.
pub(crate) fn can_unlock_extension(title: &TitleField) -> bool {
    let split = ext_split(title.field.text());
    !title.ext_unlocked && split < title.field.len() && title.field.cursor().position.get() == split
}

/// The single commit chokepoint: whether the title may release focus. A
/// commit with an unchanged name is `Accepted` outright — a plain refocus,
/// never a rename of a file onto its own path. Otherwise `rename::begin`
/// decides every refusal and owns the whole workflow from here.
/// `committed` is deliberately NOT advanced on the way out: it is the name
/// the file actually has, moving only once a rename has really landed
/// (`rename::bind_to` reseeds the field) — advancing it optimistically
/// here would make `Esc` revert to a name no file has.
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
        let vfs = std::sync::Arc::new(rune_vfs::Mem::new());
        let launch = crate::resolved::ResolvedPath::resolve(
            vfs.as_ref(),
            std::path::Path::new("/root/a.md"),
        )
        .expect("the launch path resolves");
        let mut app = crate::app::App::new(
            rune_core::buffer::Buffer::new("body"),
            Some(launch),
            vfs,
            None,
        );
        app.title.seed("a.md");
        let mut effects = Effects::default();
        assert_eq!(on_blur(&mut app, &mut effects), rename::Commit::Accepted);
        assert!(effects.cmds.is_empty());
    }
}
