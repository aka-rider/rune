use std::path::Path;

use rune_syntax::DocumentKind;

/// Deliberately calls the compile-free `detect`/`lang::resolve` path, never
/// the query-compiling registry getter, so no tree-sitter query compiles on
/// this UI thread.
pub(crate) fn kind_for(path: Option<&Path>, content: &str) -> DocumentKind {
    let Some(path) = path else {
        return DocumentKind::Markdown;
    };
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        if is_image_extension(ext) {
            return DocumentKind::Image;
        }
        if ext.eq_ignore_ascii_case("md") {
            return DocumentKind::Markdown;
        }
    }
    match rune_ts::detect(Some(path), content) {
        Some(rune_ts::Detected::Markdown) => DocumentKind::Markdown,
        Some(rune_ts::Detected::Lang(id)) => DocumentKind::Code(id),
        None => DocumentKind::Plain,
    }
}

fn is_image_extension(ext: &str) -> bool {
    rune_image::decode::extensions()
        .iter()
        .any(|known| known.eq_ignore_ascii_case(ext))
}

pub fn is_image_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(is_image_extension)
}

#[derive(Debug, PartialEq, Eq)]
pub enum Hydration {
    NoChange,
    /// Journals one synthetic bridge `Step` before replacing the buffer, so
    /// `⌘Z` can still reach the pre-recovery content.
    Adopted,
    Refused(&'static str),
}

/// Delegates to `rune_core::is_suspicious_shrink`, the same chokepoint
/// `rune-db`'s disk-read confirmation gate uses, so the two can never
/// disagree on what counts as a destructive async shrink (a
/// watcher/IME/dictation reset caught mid-write) that would otherwise
/// silently discard content the user can see on screen.
pub(crate) fn is_suspicious_shrink(disk_content: &str, recovered: &str) -> bool {
    rune_core::is_suspicious_shrink(disk_content.len(), recovered.len())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use rune_syntax::LangId;

    fn bash() -> LangId {
        LangId::from_name("bash").unwrap()
    }

    #[test]
    fn image_extension_resolves_to_image_kind() {
        assert_eq!(kind_for(Some(Path::new("a.png")), ""), DocumentKind::Image);
    }

    #[test]
    fn no_image_extension_also_resolves_as_a_code_language() {
        for ext in rune_image::decode::extensions() {
            assert!(
                rune_ts::lang::resolve(ext).is_none(),
                "image extension {ext:?} must not also resolve as a code language"
            );
        }
    }

    #[test]
    fn markdown_extension_still_wins_over_no_language() {
        assert_eq!(
            kind_for(Some(Path::new("a.md")), ""),
            DocumentKind::Markdown
        );
    }

    #[test]
    fn no_path_stays_markdown() {
        assert_eq!(kind_for(None, "hello"), DocumentKind::Markdown);
    }

    #[test]
    fn dotfile_resolves_via_whole_filename() {
        assert_eq!(
            kind_for(Some(Path::new(".zshrc")), ""),
            DocumentKind::Code(bash())
        );
    }

    #[test]
    fn extensionless_readme_is_markdown() {
        assert_eq!(
            kind_for(Some(Path::new("README")), ""),
            DocumentKind::Markdown
        );
    }

    #[test]
    fn extensionless_shebang_resolves_via_interpreter() {
        assert_eq!(
            kind_for(Some(Path::new("deploy")), "#!/bin/bash\n"),
            DocumentKind::Code(bash())
        );
    }

    #[test]
    fn md_extension_beats_a_modeline_in_content() {
        assert_eq!(
            kind_for(Some(Path::new("notes.md")), "# vim: ft=python\n"),
            DocumentKind::Markdown
        );
    }

    #[test]
    fn image_extension_beats_a_modeline_in_content() {
        assert_eq!(
            kind_for(Some(Path::new("a.png")), "# vim: ft=rust\n"),
            DocumentKind::Image
        );
    }

    #[test]
    fn unidentifiable_content_stays_plain() {
        assert_eq!(
            kind_for(Some(Path::new("mystery")), "just words\n"),
            DocumentKind::Plain
        );
    }

    #[test]
    fn is_image_path_agrees_with_kind_for() {
        for ext in rune_image::decode::extensions() {
            let p = std::path::PathBuf::from(format!("a.{ext}"));
            assert_eq!(
                is_image_path(&p),
                kind_for(Some(&p), "") == DocumentKind::Image
            );
            assert!(is_image_path(&p));
        }
        assert!(!is_image_path(Path::new("a.rs")));
        assert!(!is_image_path(Path::new("mystery")));
    }
}
