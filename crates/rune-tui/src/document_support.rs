//! Small, standalone helpers used by `Document` (split out of `document.rs`
//! per §1.6): the path-to-`DocumentKind` derivation `bind_path` calls, and
//! the hydration-adoption outcome type `Document::hydrate` returns plus its
//! destructive-shrink guard. None of these depend on `Document`'s own
//! fields — each is a pure function/type `document.rs`'s `impl Document`
//! calls into.

use std::path::Path;

use rune_syntax::DocumentKind;

/// Derives the producer a path should use (plan WP4.S4/WP4.S5): no path at
/// all (an untitled draft) stays `Markdown`; an extension `rune_image::
/// decode::extensions()` advertises becomes `Image` — checked
/// BEFORE the `.md` arm and before `rune_ts::lang::resolve`, since a still-
/// image format's extension (e.g. `.svg` when the `svg` feature is on)
/// could otherwise shadow a code language; a `.md` extension stays
/// `Markdown`; an extension `rune_ts::lang::resolve` recognises becomes
/// `Code`; anything else is `Plain`. Deliberately calls the compile-free
/// `lang::resolve`, never the query-compiling registry getter — `resolve`
/// is a pure `&'static` table lookup with no tree-sitter call at all, so no
/// query compilation happens on this (the UI) thread.
pub(crate) fn kind_for(path: Option<&Path>) -> DocumentKind {
    let Some(path) = path else {
        return DocumentKind::Markdown;
    };
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) if is_image_extension(ext) => DocumentKind::Image,
        Some(ext) if ext.eq_ignore_ascii_case("md") => DocumentKind::Markdown,
        Some(ext) => match rune_ts::lang::resolve(ext) {
            Some(name) => DocumentKind::Code(name),
            None => DocumentKind::Plain,
        },
        None => DocumentKind::Plain,
    }
}

fn is_image_extension(ext: &str) -> bool {
    rune_image::decode::extensions()
        .iter()
        .any(|known| known.eq_ignore_ascii_case(ext))
}

/// Whether `path` would open as `DocumentKind::Image` (plan WP4.S8) — the
/// one predicate `rune-cli`'s bootstrap needs to route the first
/// positional through `workspace::open_path` instead of `load_buffer`,
/// without making `rune_syntax::DocumentKind` a dependency of `rune-cli`
/// just to compare `kind_for`'s output against one variant.
pub fn is_image_path(path: &Path) -> bool {
    kind_for(Some(path)) == DocumentKind::Image
}

/// The outcome of [`crate::document::Document::hydrate`] — the shared
/// hydration-adoption chokepoint (plan WP5.S2).
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
/// is not a legitimate recovery — it is the CONSTITUTION §1.3 "destructive
/// async edit" pattern (a watcher/IME/dictation reset caught mid-write), and
/// adopting it would silently discard content the user can see on screen.
/// `disk_content` empty has nothing to protect, so it never trips this.
pub(crate) fn is_suspicious_shrink(disk_content: &str, recovered: &str) -> bool {
    !disk_content.is_empty() && recovered.len() * 2 < disk_content.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_extension_resolves_to_image_kind() {
        assert_eq!(kind_for(Some(Path::new("a.png"))), DocumentKind::Image);
    }

    /// Plan WP4 Done-when: no image extension may also resolve through
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
        assert_eq!(kind_for(Some(Path::new("a.md"))), DocumentKind::Markdown);
    }

    #[test]
    fn no_path_stays_markdown() {
        assert_eq!(kind_for(None), DocumentKind::Markdown);
    }
}
