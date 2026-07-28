//! The title row: the active document's display name plus a dirty dot, and
//! — new here — the editable [`TitleField`] that a rename types into.
//!
//! Two rendering modes, one function:
//!
//! - **Unfocused** (`app.focus != Pane::Title`): `Document::file_name()`
//!   plus ` •` when dirty. Unchanged, a pure function of `&App`.
//! - **Focused**: [`TitleField`]'s own `text` with a block cursor. The field
//!   holds the file's **stem**, not its full name — Go's title does the
//!   same (`workspace_view_switch.go:128`:
//!   `TrimSuffix(Base(path), ".md")`), and the rename target is rebuilt as
//!   `<parent>/<text>.md` (`workspace_update.go:82`). The extension is
//!   managed, never typed, so it cannot be accidentally deleted into a file
//!   rune would then refuse to reopen.
//!
//! `TitleField` is the only state this module owns, and it is genuinely
//! this component's own: `text`/`cursor`/`committed` are exactly what
//! `draw` below renders (§2.1). The rename *workflow* it kicks off lives in
//! `rename.rs` — a multi-step I/O sequence with three different renderers
//! (this module the name, `footer` the prompt, `banner` the modal slot), so
//! it belongs to no single child.
//!
//! The field is **unjournaled** (§12: "the title field is unjournaled — a
//! rename is one atomic bind"): typing here never touches the document
//! buffer, never appends a journal event, and never marks anything dirty.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::App;
use crate::document::Document;
use crate::keymap::{KeyCode, KeyInput, KeyOutcome};
use crate::pane::Pane;
use crate::rename;
use crate::runtime::Effects;
use crate::styles;

/// The extension every rune document carries. The title field edits the
/// stem; this is re-appended when the typed name becomes a real path.
pub const MARKDOWN_EXT: &str = "md";

/// Characters a file name may never contain. `/` is the path separator (a
/// typed `a/b` would silently rename into a different directory — or fail
/// confusingly); the rest are rejected because they are hostile on the
/// network volumes and archive formats a `.md` vault routinely crosses,
/// matching Go's `invalidFileNameChars` (`title.go`). `\0` and every other
/// control character are rejected via `char::is_control` below.
const INVALID_NAME_CHARS: &[char] = &['/', '\\', ':', '*', '?', '"', '<', '>'];

/// The editable title. One field on `App`, reseeded at every document
/// switch so it always describes whatever document is actually showing.
///
/// `cursor` is a **byte** offset into `text` (§1.5) — every mutation below
/// moves it by `ch.len_utf8()` or to a `char_indices` boundary, never by a
/// rune count, so a multi-byte name can never be split mid-codepoint.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TitleField {
    /// What the user has typed — the file's stem, without its extension.
    pub text: String,
    /// Byte offset of the insertion point within `text`.
    pub cursor: usize,
    /// The last committed name. `Esc` reverts to it, and a commit that
    /// doesn't change it is a no-op rather than a rename of a file to its
    /// own name.
    pub committed: String,
}

impl TitleField {
    /// Points the field at `name` (a stem) and puts the cursor at the end —
    /// the natural place to start editing an existing name. Called at every
    /// document switch (`workspace::switch_to`) so the field can never
    /// describe a document that is no longer showing.
    pub fn seed(&mut self, name: &str) {
        self.text = name.to_string();
        self.committed = self.text.clone();
        self.cursor = self.text.len();
    }

    /// Throws away the in-progress edit. `Esc`'s behavior.
    pub fn revert(&mut self) {
        self.text = self.committed.clone();
        self.cursor = self.text.len();
    }

    /// Accepts `text` as the new committed name — called once a rename has
    /// actually landed, never optimistically at keypress time.
    pub fn accept(&mut self) {
        self.committed = self.text.clone();
    }

    /// Replaces `text` with a name the user must fix (a failed rename
    /// refocuses the field holding what they typed, never the old
    /// committed name — the typed name is the thing worth keeping).
    pub fn set_text(&mut self, text: &str) {
        self.text = text.to_string();
        self.cursor = self.text.len();
    }

    fn insert(&mut self, ch: char) {
        self.text.insert(self.cursor, ch);
        self.cursor += ch.len_utf8();
    }

    /// Deletes the character *before* the cursor, moving the cursor back to
    /// that character's own start byte — never `cursor - 1`, which would
    /// land mid-codepoint on a multi-byte name.
    fn delete_left(&mut self) {
        let Some(prev) = self.prev_boundary() else {
            return;
        };
        self.text.replace_range(prev..self.cursor, "");
        self.cursor = prev;
    }

    fn delete_right(&mut self) {
        let Some(next) = self.next_boundary() else {
            return;
        };
        self.text.replace_range(self.cursor..next, "");
    }

    fn prev_boundary(&self) -> Option<usize> {
        self.text[..self.cursor]
            .char_indices()
            .next_back()
            .map(|(i, _)| i)
    }

    fn next_boundary(&self) -> Option<usize> {
        self.text[self.cursor..]
            .chars()
            .next()
            .map(|ch| self.cursor + ch.len_utf8())
    }
}

/// The stem the field should show for `doc`: its file name minus the
/// extension, or the empty string for a pathless draft (there is no name to
/// edit yet, and seeding the `"[No Name]"` display placeholder as editable
/// text would let the user rename a file to literally that).
pub fn stem_for(doc: &Document) -> String {
    doc.file_path
        .as_ref()
        .and_then(|p| p.file_stem())
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Whether `name` is usable as a file stem. Rejects the empty string (there
/// would be no file left to name), any [`INVALID_NAME_CHARS`], and every
/// control character. Not a security boundary — a refusal here is a UX
/// courtesy; the atomic no-clobber `rename_excl` is what actually protects
/// the destination.
pub fn is_valid_stem(name: &str) -> bool {
    !name.is_empty()
        && !name
            .chars()
            .any(|ch| ch.is_control() || INVALID_NAME_CHARS.contains(&ch))
}

/// Stage 3 for `Pane::Title` (§3.3): every key is consumed here — a
/// keystroke aimed at a file name must never reach the buffer, which is
/// what the fuzzer's `PANE-NO-BLEED` invariant asserts.
///
/// `Enter`/`Down` commit; `Esc` reverts and returns focus to the editor;
/// arrows/Home/End move; Backspace/Delete edit; printable characters
/// insert after filtering. Anything else is a consumed no-op.
pub fn handle_key(app: &mut App, key: KeyInput, effects: &mut Effects) -> KeyOutcome {
    match key.code {
        KeyCode::Enter | KeyCode::Down => {
            commit(app, effects);
        }
        KeyCode::Escape => {
            app.title.revert();
            app.focus = Pane::Editor;
        }
        KeyCode::Left => {
            if let Some(prev) = app.title.prev_boundary() {
                app.title.cursor = prev;
            }
        }
        KeyCode::Right => {
            if let Some(next) = app.title.next_boundary() {
                app.title.cursor = next;
            }
        }
        KeyCode::Home => app.title.cursor = 0,
        KeyCode::End => app.title.cursor = app.title.text.len(),
        KeyCode::Backspace => app.title.delete_left(),
        KeyCode::Delete => app.title.delete_right(),
        KeyCode::Char(ch)
            if !key.mods.ctrl
                && !key.mods.alt
                && !key.mods.sup
                && !ch.is_control()
                && !INVALID_NAME_CHARS.contains(&ch) =>
        {
            app.title.insert(ch);
        }
        _ => {}
    }
    KeyOutcome::Consumed
}

/// Commits the typed name if the title is focused, then returns focus to
/// the editor. `pane::handle_global_command` calls this as its FIRST
/// statement — one hoisted gate, so ⌘S (or any other global chord) pressed
/// mid-rename commits the name first rather than saving under the old one
/// or silently discarding the edit.
pub fn finalize_if_focused(app: &mut App, effects: &mut Effects) {
    if app.focus == Pane::Title {
        commit(app, effects);
    }
}

/// A commit with an unchanged name is a plain refocus, never a rename of a
/// file onto its own path.
///
/// `rename::begin` decides every refusal (read-only, a save in flight, an
/// invalid name, a rename already in progress) and owns the whole workflow
/// from here. `committed` is deliberately NOT advanced on the way out: it
/// is the name the file actually has, and it moves only once a rename has
/// really landed (`rename::bind_to` reseeds the field). Advancing it
/// optimistically here would make `Esc` revert to a name no file has.
fn commit(app: &mut App, effects: &mut Effects) {
    if app.title.text == app.title.committed {
        app.focus = Pane::Editor;
        return;
    }
    rename::begin(app, effects);
}

/// Renders `<name>` (styled `styles::title_text`) followed by ` •` when the
/// active document is dirty, or — when the title has focus — the editable
/// field with a block cursor. Pure function of `&App` in both modes: it
/// reads `app.active_doc()`/`app.title` fresh every call and caches
/// nothing, so drawing twice produces identical output.
pub fn draw(app: &App, area: Rect, frame: &mut Frame) {
    let spans = if app.focus == Pane::Title {
        field_spans(&app.title)
    } else {
        let doc = app.active_doc();
        let mut spans = vec![Span::styled(
            doc.file_name().to_string(),
            styles::title_text(),
        )];
        if doc.is_dirty() {
            spans.push(Span::styled(" \u{2022}", styles::error()));
        }
        spans
    };
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// The focused field: text before the cursor, the cursor cell (the
/// character under it, or a space at end-of-text), then the remainder.
/// Slicing is by BYTE offset — `cursor` is a byte offset and is always kept
/// on a `char` boundary by the mutators above (§1.5).
fn field_spans(field: &TitleField) -> Vec<Span<'static>> {
    let base = styles::title_text();
    let cursor_style: Style = base.add_modifier(Modifier::REVERSED);

    let (before, rest) = field.text.split_at(field.cursor.min(field.text.len()));
    let mut chars = rest.chars();
    let (at, after) = match chars.next() {
        Some(ch) => (ch.to_string(), chars.as_str().to_string()),
        None => (" ".to_string(), String::new()),
    };

    vec![
        Span::styled(before.to_string(), base),
        Span::styled(at, cursor_style),
        Span::styled(after, base),
    ]
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use rune_core::buffer::Buffer;
    use rune_vfs::Mem;
    use std::sync::Arc;

    fn app_for(content: &str) -> App {
        App::new(Buffer::new(content), None, Arc::new(Mem::new()), None)
    }

    fn draw_line(app: &App, width: u16) -> String {
        let backend = TestBackend::new(width, 1);
        let mut terminal = Terminal::new(backend).expect("terminal construction");
        terminal
            .draw(|frame| draw(app, frame.area(), frame))
            .expect("draw");
        let buf = terminal.backend().buffer().clone();
        let mut s = String::new();
        for x in 0..width {
            if let Some(cell) = buf.cell((x, 0)) {
                s.push_str(cell.symbol());
            }
        }
        s
    }

    #[test]
    fn no_name_placeholder_when_pathless() {
        let app = app_for("hello");
        assert!(draw_line(&app, 40).contains("[No Name]"));
    }

    #[test]
    fn dirty_dot_appears_only_when_dirty() {
        let mut app = app_for("hello");
        assert!(!draw_line(&app, 40).contains('\u{2022}'));
        app.active_doc_mut().mark_dirty_from_hydration();
        assert!(draw_line(&app, 40).contains('\u{2022}'));
    }

    /// The focused field shows what was TYPED, not the document's own
    /// file name — that difference is the whole point of the mode.
    #[test]
    fn focused_field_renders_the_typed_text() {
        let mut app = app_for("hello");
        app.title.seed("notes");
        app.focus = Pane::Title;
        assert!(draw_line(&app, 40).starts_with("notes"));
    }

    /// Render purity (§5.2): drawing twice must produce identical output.
    #[test]
    fn drawing_a_focused_field_twice_is_identical() {
        let mut app = app_for("hello");
        app.title.seed("notes");
        app.focus = Pane::Title;
        assert_eq!(draw_line(&app, 40), draw_line(&app, 40));
    }

    /// Byte-offset cursor arithmetic on a multi-byte name (§1.5): every
    /// motion and deletion must land on a `char` boundary.
    #[test]
    fn cursor_motion_and_deletion_stay_on_char_boundaries() {
        let mut field = TitleField::default();
        field.seed("héllo");
        assert_eq!(field.cursor, "héllo".len(), "seed lands at the byte end");

        field.cursor = 0;
        for _ in 0..2 {
            let next = field.next_boundary().expect("next");
            field.cursor = next;
        }
        assert_eq!(field.cursor, 3, "'h' is 1 byte, 'é' is 2");

        field.delete_left();
        assert_eq!(field.text, "hllo");
        assert_eq!(field.cursor, 1);

        field.delete_right();
        assert_eq!(field.text, "hlo");
        assert_eq!(field.cursor, 1);
    }

    #[test]
    fn invalid_stems_are_rejected() {
        assert!(is_valid_stem("notes"));
        assert!(is_valid_stem("h\u{e9}llo world"));
        assert!(!is_valid_stem(""));
        assert!(!is_valid_stem("a/b"));
        assert!(!is_valid_stem("a:b"));
        assert!(!is_valid_stem("a\u{0}b"));
    }
}
