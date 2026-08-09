//! Explorer row glyphs. `IconTier::Unicode` gets no icon column at all — a
//! deliberate product decision, not a missing-glyph gap — so [`icon`]
//! returns `None` unconditionally on that tier and the caller renders no
//! icon at all. `IconTier::Nerd` resolves through a short ladder that
//! mirrors `rune_ts::detect`'s (directory, then whole filename, then
//! extension, then language) minus the content-dependent rungs, since the
//! Explorer has no file contents to read.
//!
//! Every codepoint below is a Nerd Fonts v3 `nf-md-*` (Material Design
//! Icons) glyph, matching the family `rune_md::icons::IconSet::nerd()`
//! already uses for its heading glyphs — never mixed with `nf-dev-*` or
//! `nf-seti-*`, which would read as a mismatched family. Each entry names
//! its nf-md glyph so a human can re-verify the codepoint against the
//! Nerd Fonts cheat sheet without re-deriving it; the codepoint itself is
//! not memorable and must never be retyped by hand from memory.

use rune_vfs::DirEntry;

use crate::theme::icons::IconTier;

/// nf-md-folder.
const FOLDER: &str = "\u{f024b}";
/// nf-md-file_outline — the fallback for any file whose filename,
/// extension, and language all fail to resolve a glyph.
const GENERIC_FILE: &str = "\u{f0224}";
/// nf-md-language_markdown.
const MARKDOWN: &str = "\u{f0354}";

/// Whole-filename table, case-insensitive, exact match only — no globs, no
/// prefixes. `README`/`LICENSE` are deliberately absent: `README.md` must
/// resolve through the `.md` extension rung to the markdown glyph, and a
/// filename rung that shadowed the extension rung would be an unstated
/// precedence rule.
const FILENAME_GLYPHS: &[(&str, &str)] = &[
    (".gitignore", GIT),
    (".gitmodules", GIT),
    (".gitattributes", GIT),
    ("Makefile", COG),
    ("Dockerfile", DOCKER),
    ("Cargo.lock", LOCK),
    (".zshrc", CONSOLE),
    (".bashrc", CONSOLE),
];

/// nf-md-git.
const GIT: &str = "\u{f02a2}";
/// nf-md-cog — build/automation glyph for `Makefile`; nf-md has no
/// dedicated makefile glyph.
const COG: &str = "\u{f0493}";
/// nf-md-docker.
const DOCKER: &str = "\u{f0868}";
/// nf-md-lock.
const LOCK: &str = "\u{f033e}";
/// nf-md-console — shell rc file.
const CONSOLE: &str = "\u{f018d}";

/// `rune_ts::lang::LANGUAGES` name to nf-md glyph. Partial by design: nf-md
/// has no counterpart for `toml`, `yaml`, `tsx`, or `sql` — those four fall
/// through to [`GENERIC_FILE`] rather than borrowing a glyph from another
/// icon family.
const LANGUAGE_GLYPHS: &[(&str, &str)] = &[
    ("rust", "\u{f1617}"),       // nf-md-language_rust
    ("json", "\u{f0626}"),       // nf-md-code_json
    ("bash", "\u{f1183}"),       // nf-md-bash
    ("python", "\u{f0320}"),     // nf-md-language_python
    ("javascript", "\u{f031e}"), // nf-md-language_javascript
    ("go", "\u{f07d3}"),         // nf-md-language_go
    ("html", "\u{f031d}"),       // nf-md-language_html5
    ("css", "\u{f031c}"),        // nf-md-language_css3
    ("c", "\u{f0671}"),          // nf-md-language_c
    ("cpp", "\u{f0672}"),        // nf-md-language_cpp
    ("typescript", "\u{f06e6}"), // nf-md-language_typescript
    ("java", "\u{f0b37}"),       // nf-md-language_java
    ("csharp", "\u{f031b}"),     // nf-md-language_csharp
    ("php", "\u{f031f}"),        // nf-md-language_php
    ("ruby", "\u{f0d2d}"),       // nf-md-language_ruby
    ("terraform", "\u{f1062}"),  // nf-md-terraform
    ("kotlin", "\u{f1219}"),     // nf-md-language_kotlin
    ("swift", "\u{f06e5}"),      // nf-md-language_swift
];

/// The glyph for one Explorer row, or `None` on a tier with no icon font —
/// the caller renders no icon column at all in that case.
pub fn icon(tier: IconTier, entry: &DirEntry) -> Option<&'static str> {
    if tier == IconTier::Unicode {
        return None;
    }
    Some(resolve_nerd(entry))
}

fn resolve_nerd(entry: &DirEntry) -> &'static str {
    if entry.is_dir {
        return FOLDER;
    }
    if let Some(glyph) = FILENAME_GLYPHS
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(&entry.name))
        .map(|(_, glyph)| *glyph)
    {
        return glyph;
    }
    let ext = std::path::Path::new(&entry.name)
        .extension()
        .and_then(|e| e.to_str());
    if let Some(ext) = ext {
        if ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("markdown") {
            return MARKDOWN;
        }
        if let Some(lang) = rune_ts::resolve(ext)
            && let Some(glyph) = LANGUAGE_GLYPHS
                .iter()
                .find(|(name, _)| *name == lang)
                .map(|(_, glyph)| *glyph)
        {
            return glyph;
        }
    }
    GENERIC_FILE
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn file(name: &str) -> DirEntry {
        DirEntry {
            name: name.to_string(),
            path: PathBuf::from(name),
            is_dir: false,
        }
    }

    fn dir(name: &str) -> DirEntry {
        DirEntry {
            name: name.to_string(),
            path: PathBuf::from(name),
            is_dir: true,
        }
    }

    #[test]
    fn rust_markdown_makefile_and_unknown_extension_resolve_to_distinct_glyphs() {
        let rust = icon(IconTier::Nerd, &file("main.rs")).expect("nerd tier has an icon");
        let md = icon(IconTier::Nerd, &file("notes.md")).expect("nerd tier has an icon");
        let makefile = icon(IconTier::Nerd, &file("Makefile")).expect("nerd tier has an icon");
        let unknown = icon(IconTier::Nerd, &file("data.xyz")).expect("nerd tier has an icon");
        let all = [rust, md, makefile, unknown];
        for i in 0..all.len() {
            for j in (i + 1)..all.len() {
                assert_ne!(all[i], all[j], "glyphs at {i} and {j} collide");
            }
        }
    }

    #[test]
    fn unknown_extension_is_the_generic_file_glyph() {
        assert_eq!(icon(IconTier::Nerd, &file("data.xyz")), Some(GENERIC_FILE));
    }

    #[test]
    fn readme_md_resolves_to_the_markdown_glyph_not_a_readme_glyph() {
        assert_eq!(icon(IconTier::Nerd, &file("README.md")), Some(MARKDOWN));
    }

    #[test]
    fn a_directory_resolves_to_the_folder_glyph_regardless_of_name() {
        assert_eq!(icon(IconTier::Nerd, &dir("src")), Some(FOLDER));
        assert_eq!(
            icon(IconTier::Nerd, &dir("Makefile")),
            Some(FOLDER),
            "a directory literally named Makefile is still a folder"
        );
    }

    #[test]
    fn unicode_tier_returns_none_for_every_case_above() {
        for entry in [
            file("main.rs"),
            file("notes.md"),
            file("Makefile"),
            file("data.xyz"),
            file("README.md"),
            dir("src"),
        ] {
            assert_eq!(icon(IconTier::Unicode, &entry), None);
        }
    }

    /// LAYOUT invariant only: every glyph in the table measures exactly 1
    /// cell through the one width chokepoint, so the caller's fixed
    /// two-cell `"{icon} "` column never misaligns. This cannot detect a
    /// wrong codepoint — `unicode-width` returns the same value for every
    /// private-use-area codepoint, correct or not — it only guards layout.
    #[test]
    fn every_table_glyph_measures_one_cell() {
        let mut all = vec![FOLDER, GENERIC_FILE, MARKDOWN];
        all.extend(FILENAME_GLYPHS.iter().map(|(_, g)| *g));
        all.extend(LANGUAGE_GLYPHS.iter().map(|(_, g)| *g));
        for glyph in all {
            assert_eq!(
                crate::width::display_width(glyph),
                1,
                "{glyph:?} must measure 1 cell"
            );
        }
    }
}
