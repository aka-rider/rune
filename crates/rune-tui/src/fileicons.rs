use rune_vfs::{DirEntry, FileKind};

use crate::theme::icons::IconTier;

// Every codepoint below is a Nerd Fonts v3 `nf-md-*` glyph — never
// `nf-dev-*`/`nf-seti-*`, a mismatched family — named in a comment so it
// can be re-verified against the Nerd Fonts cheat sheet rather than
// retyped from memory.
const FOLDER: &str = "\u{f024b}"; // nf-md-folder
const GENERIC_FILE: &str = "\u{f0224}"; // nf-md-file_outline
const MARKDOWN: &str = "\u{f0354}"; // nf-md-language_markdown

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

const GIT: &str = "\u{f02a2}"; // nf-md-git
// nf-md-cog: nf-md has no dedicated makefile glyph.
const COG: &str = "\u{f0493}";
const DOCKER: &str = "\u{f0868}"; // nf-md-docker
const LOCK: &str = "\u{f033e}"; // nf-md-lock
const CONSOLE: &str = "\u{f018d}"; // nf-md-console

// Partial by design: nf-md has no counterpart for `toml`, `yaml`, `tsx`,
// or `sql` — those four fall through to `GENERIC_FILE` rather than
// borrowing a glyph from another icon family.
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

pub fn icon(tier: IconTier, entry: &DirEntry) -> Option<&'static str> {
    if tier == IconTier::Unicode {
        return None;
    }
    Some(resolve_nerd(entry))
}

// Mirrors `rune_ts::detect`'s ladder — directory, then whole filename, then
// extension, then language — minus the content-dependent rungs, since the
// Explorer has no file contents to read.
fn resolve_nerd(entry: &DirEntry) -> &'static str {
    if entry.kind == FileKind::Dir {
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
                .find(|(name, _)| *name == lang.name())
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
            kind: FileKind::File,
            link: rune_vfs::Link::No,
        }
    }

    fn dir(name: &str) -> DirEntry {
        DirEntry {
            name: name.to_string(),
            path: PathBuf::from(name),
            kind: FileKind::Dir,
            link: rune_vfs::Link::No,
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

    // Layout invariant only: this cannot catch a wrong codepoint —
    // `unicode-width` reports the same width for every private-use-area
    // codepoint, correct or not.
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
