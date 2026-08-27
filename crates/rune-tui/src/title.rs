use std::ops::Range;

use crate::document::Document;

pub mod keys;

pub use keys::{handle_key, on_blur};

// Used only by `name_for`'s draft fallback — a typed name is never
// re-appended with this once it reaches `rename.rs`: the rename target is
// the typed name, verbatim.
const MARKDOWN_EXT: &str = "md";

const INVALID_NAME_CHARS: &[char] = &['/', '\\', ':', '*', '?', '"', '<', '>'];

/// The single predicate every entry point shares — the typed-character
/// guard, the paste sanitizer, and `is_valid_name`'s whole-string check —
/// so the three can never drift into disagreeing about what a name may
/// contain. Rejecting `/` and `\` here is what keeps `rename::target_path`'s
/// `Path::join` inside the parent directory: `join` with an absolute path
/// REPLACES the base rather than appending to it.
pub fn is_name_char(ch: char) -> bool {
    !ch.is_control() && !INVALID_NAME_CHARS.contains(&ch)
}

pub struct TitleField {
    field: crate::field::TextField,
    committed: String,
    ext_unlocked: bool,
}

impl TitleField {
    pub fn seed(&mut self, name: &str) {
        self.committed = name.to_string();
        self.place(name);
    }

    pub fn revert(&mut self) {
        let committed = self.committed.clone();
        self.place(&committed);
    }

    /// Places `text` in the field without touching `committed` — the
    /// shared primitive `seed`/`revert` are both built on.
    pub fn set_text(&mut self, text: &str) {
        self.place(text);
    }

    // An extensionless name must ALSO start unlocked, not just an empty
    // stem: the window is re-derived from the live text on every keystroke,
    // so on an extensionless name the first `.` the user types becomes the
    // split and would shrink the window to exclude the caret's own
    // position — every following character then clamps back in front of
    // the dot, turning `README` + `.md` into `READMEmd.` and committing it
    // on blur. Once a real extension exists the split can only ever move
    // to or past the caret, so the caret can never be stranded outside the
    // window.
    fn place(&mut self, name: &str) {
        self.field.set_text(name);
        let split = ext_split(name);
        self.field.set_cursor(split, split);
        self.ext_unlocked = split == 0 || split == name.len();
    }

    pub fn text(&self) -> &str {
        self.field.text()
    }

    pub fn committed(&self) -> &str {
        &self.committed
    }

    pub fn ext_unlocked(&self) -> bool {
        self.ext_unlocked
    }

    pub fn field(&self) -> &crate::field::TextField {
        &self.field
    }

    pub fn field_mut(&mut self) -> &mut crate::field::TextField {
        &mut self.field
    }

    /// Recomputed fresh from the live text on every call, never stored, so
    /// it can never drift out of sync with an edit that moved the dot.
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

pub fn ext_split(name: &str) -> usize {
    name.rfind('.').unwrap_or(name.len())
}

/// Seeding falls back to the draft default (an empty stem plus `.md`) for
/// a pathless document rather than its `"[No Name]"`/`"Help"` display
/// placeholder: neither is a typeable file name, and seeding one as
/// editable text would let the user rename a file to literally that. A
/// document that DOES have a `file_path` still prefers its `display_name`
/// when one is set — currently only merge mode's own retitle — so this
/// name can never disagree with what the tab bar shows for the same
/// document mid-merge.
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

/// Not a security boundary — a refusal here is a UX courtesy; the atomic
/// no-clobber `rename_excl` is what actually protects the destination.
pub fn is_valid_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.chars().any(|ch| !is_name_char(ch))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use rune_core::buffer::Buffer;
    use rune_vfs::{Mem, VfsTestExt};
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
        assert_eq!(field.field().cursor().position.get(), 6);
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
