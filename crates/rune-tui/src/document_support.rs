//! Small, standalone helpers used by `Document` (split out of `document.rs`
//! per the 500-line budget): the path-to-`DocumentKind` derivation `bind_path` calls, and
//! the hydration-adoption outcome type `Document::hydrate` returns plus its
//! destructive-shrink guard. None of these depend on `Document`'s own
//! fields — each is a pure function/type `document.rs`'s `impl Document`
//! calls into.

use std::path::Path;

use rune_syntax::DocumentKind;

/// Derives the producer a path (plus the document's current content) should
/// use: no path at all (an untitled draft) stays `Markdown`; an extension
/// `rune_image::decode::extensions()` advertises becomes `Image` — checked
/// BEFORE everything else, since a still-image format's extension (e.g.
/// `.svg` when the `svg` feature is on) could otherwise shadow a code
/// language or be overridden by a modeline sitting inside a binary-ish blob;
/// a `.md` extension stays `Markdown` — checked BEFORE `rune_ts::detect`,
/// because markdown is rune's native format and a `vim: ft=` line living
/// inside a fenced code block of an otherwise-ordinary markdown document
/// must not hijack the whole document's kind. Past those two fixed arms,
/// `rune_ts::detect` walks its own ladder (modeline, then whole filename,
/// then extension, then shebang) over the path and content together, so
/// `.zshrc`, `Gemfile`, `README` and an extensionless shebang script all
/// resolve correctly; anything `detect` can't identify is `Plain`.
/// Deliberately calls the compile-free `detect`/`lang::resolve` path, never
/// the query-compiling registry getter — both are pure `&'static` table
/// lookups with no tree-sitter call at all, so no query compilation happens
/// on this (the UI) thread.
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

/// Whether `path` would open as `DocumentKind::Image` — the
/// one predicate `rune-cli`'s bootstrap needs to route the first
/// positional through `workspace::open_path` instead of `load_buffer`,
/// without making `rune_syntax::DocumentKind` a dependency of `rune-cli`
/// just to compare `kind_for`'s output against one variant. A pure
/// extension check against `rune_image::decode::extensions()` — it
/// deliberately does not route through `kind_for`'s fuller derivation,
/// since nothing past the extension can change the answer.
pub fn is_image_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(is_image_extension)
}

/// The outcome of [`crate::document::Document::hydrate`] — the shared
/// hydration-adoption chokepoint.
#[derive(Debug, PartialEq, Eq)]
pub enum Hydration {
    /// `recovered` was identical to `disk_content` — nothing to adopt.
    NoChange,
    /// `recovered` replaced the buffer, journaled as one synthetic bridge
    /// `Step` so ⌘Z reaches `disk_content`, and the buffer is now dirty.
    Adopted,
    /// Adoption was refused; the buffer is unchanged. Carries the reason
    /// for the caller to surface (a banner/status message).
    Refused(&'static str),
}

/// A `recovered` far shorter than `disk_content` (or emptying it outright)
/// is not a legitimate recovery — it is the "destructive
/// async edit" pattern (a watcher/IME/dictation reset caught mid-write), and
/// adopting it would silently discard content the user can see on screen.
/// Thin wrapper over the shared chokepoint (`rune_core::is_suspicious_shrink`)
/// every fresh-read-vs-trusted-history comparison in the app now goes
/// through, so hydration's own guard and the disk-read confirmation gate in
/// `rune-db` can never drift apart on what counts as suspicious.
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

    /// No image extension may also resolve through
    /// `rune_ts::lang::resolve` — the image arm must never shadow a code
    /// language `kind_for` would otherwise have picked for that extension.
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

    /// A dotfile with no extension (`Path::extension()` is `None` for
    /// `.zshrc`) is still identified by `rune_ts::detect`'s whole-filename
    /// step.
    #[test]
    fn dotfile_resolves_via_whole_filename() {
        assert_eq!(
            kind_for(Some(Path::new(".zshrc")), ""),
            DocumentKind::Code(bash())
        );
    }

    /// An extensionless documentation file name resolves to `Markdown`
    /// through `rune_ts::detect`'s whole-filename step, not just `.md`.
    #[test]
    fn extensionless_readme_is_markdown() {
        assert_eq!(
            kind_for(Some(Path::new("README")), ""),
            DocumentKind::Markdown
        );
    }

    /// An extensionless script with a shebang line is identified by
    /// `rune_ts::detect`'s shebang step.
    #[test]
    fn extensionless_shebang_resolves_via_interpreter() {
        assert_eq!(
            kind_for(Some(Path::new("deploy")), "#!/bin/bash\n"),
            DocumentKind::Code(bash())
        );
    }

    /// The `.md` arm is resolved from the extension BEFORE `detect` ever
    /// runs, so a modeline living inside a markdown document's content (e.g.
    /// inside a fenced code block) can never hijack the document's kind.
    #[test]
    fn md_extension_beats_a_modeline_in_content() {
        assert_eq!(
            kind_for(Some(Path::new("notes.md")), "# vim: ft=python\n"),
            DocumentKind::Markdown
        );
    }

    /// The image arm is resolved from the extension BEFORE `detect` ever
    /// runs, so a modeline can never shadow an image extension either.
    #[test]
    fn image_extension_beats_a_modeline_in_content() {
        assert_eq!(
            kind_for(Some(Path::new("a.png")), "# vim: ft=rust\n"),
            DocumentKind::Image
        );
    }

    /// Content `detect` cannot identify by any rung of its ladder still
    /// falls back to `Plain`, unchanged from before this module consulted
    /// content at all.
    #[test]
    fn unidentifiable_content_stays_plain() {
        assert_eq!(
            kind_for(Some(Path::new("mystery")), "just words\n"),
            DocumentKind::Plain
        );
    }

    /// `is_image_path` is now a standalone extension check rather than a
    /// `kind_for` comparison — it must still answer identically for every
    /// extension the image decoder advertises, and must still say no for a
    /// non-image path.
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
