//! The editable title: the [`TitleField`] a rename types into, and the
//! derived stem/extension split every other module in this family reads
//! through `window()`/`ext_unlocked()`.
//!
//! Three modules share this concern, split along what each one owns:
//! - **This file** — the field's own shape: what it holds, how it is
//!   seeded/reverted, and `ext_split`/`is_valid_name`/`name_for`, the pure
//!   functions everything else is built from.
//! - [`keys`] (`title/keys.rs`) — keystroke handling (`handle_key`) and the
//!   blur commit chokepoint (`on_blur`); re-exported here so every existing
//!   `title::handle_key`/`title::on_blur` call site keeps working.
//! - `render::title` — every span the title row ever paints. A sibling of
//!   this module, not a descendant, so it reads `TitleField` only through
//!   the public accessors below.
//!
//! The field is unjournaled at the DOCUMENT level — a rename is one atomic
//! bind: typing here never touches the document buffer, never appends to
//! the document's own journal, and never marks the document dirty. Its own
//! [`crate::field::TextField`] DOES keep an in-memory undo history (⌘Z/⇧⌘Z)
//! — that history is private to the field, never replicated to the
//! recovery store, and discarded
//! outright by every [`TitleField::seed`]/[`TitleField::set_text`].
//!
//! `TitleField` holds the FULL file name, extension included, in one
//! `TextField` rather than two separately-tracked strings (decision 1):
//! `lessrc.md` -> `lessrc` requires the dot itself to be editable, which a
//! separately-tracked stem/extension pair cannot express. The boundary
//! between the two is *derived* on every call by [`ext_split`], never
//! stored, so it can never drift out of sync with an edit that moved the
//! dot.

use std::ops::Range;

use crate::document::Document;

pub mod keys;

pub use keys::{handle_key, on_blur};

/// The extension a pathless draft seeds with: an empty stem plus the
/// extension every rune document gets, so a draft reads as immediately
/// editable and ⌘S still creates a `.md` file even if the user never
/// touches the extension. Used only by [`name_for`]'s draft fallback — a
/// typed name is never re-appended with this once it reaches `rename.rs`
/// (decision 10: the rename target is the typed name, verbatim).
const MARKDOWN_EXT: &str = "md";

/// Characters a file name may never contain. `/` is the path separator (a
/// typed `a/b` would silently rename into a different directory — or fail
/// confusingly); the rest are rejected because they are hostile on the
/// network volumes and archive formats a `.md` vault routinely crosses.
/// `\0` and every other control character are rejected via `char::is_control` at every call
/// site.
const INVALID_NAME_CHARS: &[char] = &['/', '\\', ':', '*', '?', '"', '<', '>'];

/// Whether one character may appear in a file name. The single predicate
/// every entry point shares — the typed-character guard, the paste
/// sanitizer, and `is_valid_name`'s whole-string check — so the three can
/// never drift into disagreeing about what a name may contain. Rejecting
/// `/` and `\` here is what keeps `rename::target_path`'s `Path::join`
/// inside the parent directory: `join` with an absolute path REPLACES the
/// base rather than appending to it.
pub fn is_name_char(ch: char) -> bool {
    !ch.is_control() && !INVALID_NAME_CHARS.contains(&ch)
}

/// The editable title. One field on `App`, reseeded at every document
/// switch and every focus gain so it always describes whatever document is
/// actually showing.
pub struct TitleField {
    field: crate::field::TextField,
    /// The last committed name. `Esc` reverts to it, and a commit that
    /// doesn't change it is a no-op rather than a rename of a file to its
    /// own name.
    committed: String,
    /// Whether this focus session has entered the extension. Starts
    /// UNLOCKED whenever the seeded name has an empty stem (decision 9): a
    /// dotfile or a fresh draft has nothing to fence off, and a locked
    /// zero-width window would make Home, End, Backspace, ⌥← and ⌘A all
    /// silently inert. Otherwise latches true for the rest of the focus
    /// session on the Right-at-end-of-stem gesture (`keys::handle_key`).
    ext_unlocked: bool,
}

impl TitleField {
    /// Points the field at `name` (the full file name) and commits it —
    /// the file actually has this name. Puts the cursor at the stem/
    /// extension split, the natural place to start editing an existing
    /// name, and recomputes the gate from `name` itself. Called at every
    /// document switch (`workspace::switch_to`) and every focus gain
    /// (`App::focus_title`).
    pub fn seed(&mut self, name: &str) {
        self.committed = name.to_string();
        self.place(name);
    }

    /// Throws away the in-progress edit and its undo history, returning to
    /// `committed` — `Esc`'s behavior. Recomputes the gate exactly as
    /// `seed` would, from the COMMITTED name: reverting to a dotfile-shaped
    /// name must unlock too.
    pub fn revert(&mut self) {
        let committed = self.committed.clone();
        self.place(&committed);
    }

    /// Replaces the typed text WITHOUT touching `committed` — the shared
    /// primitive `seed`/`revert` are both built on, exposed for callers
    /// that need to place new text in the field without asserting it is
    /// the file's real name.
    pub fn set_text(&mut self, text: &str) {
        self.place(text);
    }

    /// The gate exists only when the seeded name has a real extension to
    /// fence off — `0 < split < len`. Both degenerate cases start unlocked:
    /// a name with no stem (`.md`, `.gitignore`) has nothing to protect the
    /// stem from, and a name with no extension has no extension to protect.
    ///
    /// The second case is load-bearing, not a courtesy. The window is
    /// re-derived from the LIVE text on every keystroke, so on an
    /// extensionless name the first `.` the user types becomes the split
    /// and shrinks the window to exclude the caret's own position — every
    /// following character then clamps back in front of the dot, turning
    /// `README` + `.md` into `READMEmd.` and committing it on blur. Once a
    /// real extension exists the split can only ever move to or past the
    /// caret, so the caret can never be stranded outside the window.
    fn place(&mut self, name: &str) {
        self.field.set_text(name);
        let split = ext_split(name);
        self.field.set_cursor(split, split);
        self.ext_unlocked = split == 0 || split == name.len();
    }

    /// What the user has typed so far — the full name, extension included.
    pub fn text(&self) -> &str {
        self.field.text()
    }

    /// The name the file actually has right now.
    pub fn committed(&self) -> &str {
        &self.committed
    }

    /// Whether the extension gate has latched open for this focus session.
    pub fn ext_unlocked(&self) -> bool {
        self.ext_unlocked
    }

    /// The editing core, for rendering (cursor/selection) and the key
    /// layer's `apply`/`insert` calls.
    pub fn field(&self) -> &crate::field::TextField {
        &self.field
    }

    pub fn field_mut(&mut self) -> &mut crate::field::TextField {
        &mut self.field
    }

    /// The currently-editable sub-range: the whole name once unlocked,
    /// else everything before the extension. Never stored (gotcha 12) —
    /// recomputed fresh from the LIVE text on every call, so it can never
    /// drift out of sync with an edit that moved the dot.
    pub fn window(&self) -> Range<usize> {
        if self.ext_unlocked {
            0..self.field.len()
        } else {
            0..ext_split(self.field.text())
        }
    }
}

impl Default for TitleField {
    fn default() -> Self {
        TitleField {
            field: crate::field::TextField::new(""),
            committed: String::new(),
            ext_unlocked: true,
        }
    }
}

/// The byte offset where the extension begins: the last `.` in `name`, or
/// `name.len()` when there is none. Unconditional `rfind` (gotcha 10) — a
/// dotfile like `.gitignore` is therefore all extension and no stem, which
/// is exactly what keeps decision 9's empty-stem unlock rule meaningful
/// instead of a special case.
pub fn ext_split(name: &str) -> usize {
    name.rfind('.').unwrap_or(name.len())
}

/// The full name `doc` should seed its title with: the path's own
/// `file_name()`, or the draft default (an empty stem plus `.md`) for a
/// pathless document — there is no name to edit yet, and seeding the
/// `"[No Name]"` display placeholder as editable text would let the user
/// rename a file to literally that.
///
/// `display_name` overrides this ONLY when `doc` also has a real
/// `file_path` (plan WP3.S8, `[B5]`): a bound document's only current
/// source of a display-name override is merge mode's own retitle
/// (`"{file_name}: editor <-> disk"`, `merge::landing`), so respecting it
/// here is what keeps `file_name()` (the tab/title row's own name source)
/// and this seed from ever disagreeing about a document mid-merge. A
/// PATHLESS document's `display_name` (an "Untitled N" draft, the Help
/// virtual document) is deliberately NOT respected here: that placeholder
/// is not a typeable file name, and seeding it into the rename field would
/// either lose the `.md` extension the draft flow depends on or let a
/// rename target literally become "Help".
pub fn name_for(doc: &Document) -> String {
    if doc.file_path.is_some()
        && let Some(name) = doc.display_name.as_deref()
    {
        return name.to_string();
    }
    doc.file_path
        .as_ref()
        .and_then(|p| p.file_name())
        .map_or_else(
            || format!(".{MARKDOWN_EXT}"),
            |s| s.to_string_lossy().into_owned(),
        )
}

/// Whether `name` is usable as a file name. Rejects the empty string,
/// every control character, any [`INVALID_NAME_CHARS`], and the two
/// special directory entries `.`/`..` — all three newly reachable now that
/// the name includes the extension (with the gate locked, none of these
/// could ever be typed; unlocked, they are one keystroke away). Not a
/// security boundary — a refusal here is a UX courtesy; the atomic
/// no-clobber `rename_excl` is what actually protects the destination.
pub fn is_valid_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.chars().any(|ch| !is_name_char(ch))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use rune_core::buffer::Buffer;
    use rune_vfs::{Mem, Vfs};
    use std::sync::Arc;

    fn app_for(content: &str) -> crate::app::App {
        crate::app::App::new(Buffer::new(content), None, Arc::new(Mem::new()), None)
    }

    #[test]
    fn ext_split_finds_the_last_dot_or_the_end() {
        assert_eq!(ext_split("lessrc.md"), 6);
        assert_eq!(ext_split("archive.tar.gz"), 11);
        assert_eq!(ext_split("noext"), 5);
        assert_eq!(ext_split(".gitignore"), 0);
        assert_eq!(ext_split(""), 0);
    }

    #[test]
    fn seeding_a_name_with_a_stem_locks_the_gate_at_the_split() {
        let mut field = TitleField::default();
        field.seed("lessrc.md");
        assert!(!field.ext_unlocked());
        assert_eq!(field.window(), 0..6);
        assert_eq!(field.field().cursor().position, 6);
    }

    #[test]
    fn seeding_an_empty_stem_starts_unlocked() {
        let mut field = TitleField::default();
        field.seed(".md");
        assert!(field.ext_unlocked());
        assert_eq!(field.window(), 0..3);
    }

    #[test]
    fn revert_restores_the_committed_name_and_its_gate() {
        let mut field = TitleField::default();
        field.seed("lessrc.md");
        field.set_text(".foo");
        assert!(field.ext_unlocked(), "an empty stem from '.foo' unlocks");
        field.revert();
        assert_eq!(field.text(), "lessrc.md");
        assert!(
            !field.ext_unlocked(),
            "revert recomputes the gate from committed"
        );
    }

    #[test]
    fn is_valid_name_rejects_empty_dot_and_dotdot() {
        assert!(!is_valid_name(""));
        assert!(!is_valid_name("."));
        assert!(!is_valid_name(".."));
        assert!(!is_valid_name("a/b"));
        assert!(!is_valid_name("a\u{0}b"));
        assert!(is_valid_name("notes.md"));
        assert!(is_valid_name(".gitignore"));
    }

    #[test]
    fn name_for_a_pathless_draft_is_a_dotted_md() {
        let app = app_for("hello");
        assert_eq!(name_for(app.active_doc()), ".md");
    }

    #[test]
    fn name_for_a_real_path_is_the_whole_file_name() {
        let mem = Arc::new(Mem::new());
        mem.save_atomic(std::path::Path::new("/root/a.md"), b"hi")
            .expect("seed");
        let vfs: Arc<dyn rune_vfs::Vfs + Send + Sync> = mem;
        let app = crate::app::App::new(
            Buffer::new("hi"),
            Some(std::path::PathBuf::from("/root/a.md")),
            vfs,
            None,
        );
        assert_eq!(name_for(app.active_doc()), "a.md");
    }
}
