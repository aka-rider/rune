//! Compile-free language identification. This module never constructs a
//! `tree_sitter::Parser` or `tree_sitter::Query` — it is a pure, static
//! table lookup, safe to call from the UI thread on every keystroke (to
//! derive a `DocumentKind` from a file extension or a fenced code block's
//! info string) without paying for any grammar or query compilation. The
//! compiling half lives in a separate module reached only from a
//! background command.

use rune_core::assert_invariant;
use rune_syntax::LangId;
use tree_sitter::Language;

/// One language's identity plus the two closures needed to load it later —
/// never called from this module, only stored for the compiling half to
/// invoke on a worker thread.
pub struct LangDef {
    pub name: &'static str,
    pub language: fn() -> Language,
    pub highlights: fn() -> String,
}

/// The 22 supported languages, keyed by their canonical lowercase name.
pub static LANGUAGES: &[LangDef] = &[
    LangDef {
        name: "rust",
        language: || tree_sitter_rust::LANGUAGE.into(),
        highlights: || tree_sitter_rust::HIGHLIGHTS_QUERY.to_string(),
    },
    LangDef {
        name: "json",
        language: || tree_sitter_json::LANGUAGE.into(),
        highlights: || tree_sitter_json::HIGHLIGHTS_QUERY.to_string(),
    },
    LangDef {
        name: "toml",
        language: || tree_sitter_toml_ng::LANGUAGE.into(),
        highlights: || tree_sitter_toml_ng::HIGHLIGHTS_QUERY.to_string(),
    },
    LangDef {
        name: "yaml",
        language: || tree_sitter_yaml::LANGUAGE.into(),
        highlights: || tree_sitter_yaml::HIGHLIGHTS_QUERY.to_string(),
    },
    LangDef {
        name: "bash",
        language: || tree_sitter_bash::LANGUAGE.into(),
        // Bash exports the singular `HIGHLIGHT_QUERY`, unlike most other
        // grammars here — see this crate's module docs on the naming split.
        highlights: || tree_sitter_bash::HIGHLIGHT_QUERY.to_string(),
    },
    LangDef {
        name: "python",
        language: || tree_sitter_python::LANGUAGE.into(),
        highlights: || tree_sitter_python::HIGHLIGHTS_QUERY.to_string(),
    },
    LangDef {
        name: "javascript",
        language: || tree_sitter_javascript::LANGUAGE.into(),
        // Singular, like bash/c/cpp.
        highlights: || tree_sitter_javascript::HIGHLIGHT_QUERY.to_string(),
    },
    LangDef {
        name: "go",
        language: || tree_sitter_go::LANGUAGE.into(),
        highlights: || tree_sitter_go::HIGHLIGHTS_QUERY.to_string(),
    },
    LangDef {
        name: "html",
        language: || tree_sitter_html::LANGUAGE.into(),
        highlights: || tree_sitter_html::HIGHLIGHTS_QUERY.to_string(),
    },
    LangDef {
        name: "css",
        language: || tree_sitter_css::LANGUAGE.into(),
        highlights: || tree_sitter_css::HIGHLIGHTS_QUERY.to_string(),
    },
    LangDef {
        name: "c",
        language: || tree_sitter_c::LANGUAGE.into(),
        // Singular, like bash/javascript/cpp.
        highlights: || tree_sitter_c::HIGHLIGHT_QUERY.to_string(),
    },
    LangDef {
        name: "cpp",
        language: || tree_sitter_cpp::LANGUAGE.into(),
        // Singular, like bash/javascript/c.
        highlights: || tree_sitter_cpp::HIGHLIGHT_QUERY.to_string(),
    },
    LangDef {
        name: "typescript",
        language: || tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        // TypeScript's own query is a delta over JavaScript's with no
        // `; inherits:` directive, so the effective query is the
        // concatenation of both.
        highlights: || {
            format!(
                "{}\n{}",
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_typescript::HIGHLIGHTS_QUERY
            )
        },
    },
    LangDef {
        name: "tsx",
        language: || tree_sitter_typescript::LANGUAGE_TSX.into(),
        // JavaScript base + its JSX delta + TypeScript's delta, in that
        // order — mirroring `typescript` above plus JSX support.
        highlights: || {
            format!(
                "{}\n{}\n{}",
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_javascript::JSX_HIGHLIGHT_QUERY,
                tree_sitter_typescript::HIGHLIGHTS_QUERY
            )
        },
    },
    LangDef {
        name: "java",
        language: || tree_sitter_java::LANGUAGE.into(),
        highlights: || tree_sitter_java::HIGHLIGHTS_QUERY.to_string(),
    },
    LangDef {
        name: "csharp",
        language: || tree_sitter_c_sharp::LANGUAGE.into(),
        highlights: || tree_sitter_c_sharp::HIGHLIGHTS_QUERY.to_string(),
    },
    LangDef {
        name: "php",
        language: || tree_sitter_php::LANGUAGE_PHP.into(),
        highlights: || tree_sitter_php::HIGHLIGHTS_QUERY.to_string(),
    },
    LangDef {
        name: "ruby",
        language: || tree_sitter_ruby::LANGUAGE.into(),
        highlights: || tree_sitter_ruby::HIGHLIGHTS_QUERY.to_string(),
    },
    LangDef {
        name: "terraform",
        language: || tree_sitter_hcl::LANGUAGE.into(),
        // `tree-sitter-hcl` exports no query of its own; ours is
        // hand-authored offline against its grammar.
        highlights: || include_str!("../queries/terraform.scm").to_string(),
    },
    LangDef {
        name: "sql",
        language: || tree_sitter_sequel::LANGUAGE.into(),
        highlights: || tree_sitter_sequel::HIGHLIGHTS_QUERY.to_string(),
    },
    LangDef {
        name: "kotlin",
        language: || tree_sitter_kotlin_sg::LANGUAGE.into(),
        highlights: || tree_sitter_kotlin_sg::HIGHLIGHTS_QUERY.to_string(),
    },
    LangDef {
        name: "swift",
        language: || tree_sitter_swift::LANGUAGE.into(),
        highlights: || tree_sitter_swift::HIGHLIGHTS_QUERY.to_string(),
    },
    LangDef {
        name: "erlang",
        language: || tree_sitter_erlang::LANGUAGE.into(),
        highlights: || tree_sitter_erlang::HIGHLIGHTS_QUERY.to_string(),
    },
    LangDef {
        name: "haskell",
        language: || tree_sitter_haskell::LANGUAGE.into(),
        highlights: || tree_sitter_haskell::HIGHLIGHTS_QUERY.to_string(),
    },
    LangDef {
        name: "elixir",
        language: || tree_sitter_elixir::LANGUAGE.into(),
        highlights: || tree_sitter_elixir::HIGHLIGHTS_QUERY.to_string(),
    },
    LangDef {
        name: "ocaml",
        language: || tree_sitter_ocaml::LANGUAGE_OCAML.into(),
        highlights: || tree_sitter_ocaml::HIGHLIGHTS_QUERY.to_string(),
    },
    LangDef {
        name: "scala",
        language: || tree_sitter_scala::LANGUAGE.into(),
        highlights: || tree_sitter_scala::HIGHLIGHTS_QUERY.to_string(),
    },
    LangDef {
        name: "r",
        language: || tree_sitter_r::LANGUAGE.into(),
        highlights: || tree_sitter_r::HIGHLIGHTS_QUERY.to_string(),
    },
];

/// Fence info-string and file-extension spellings that don't already match
/// a canonical name in [`LANGUAGES`]. `md` is deliberately absent —
/// markdown stays comrak's.
pub static ALIASES: &[(&str, &str)] = &[
    ("rs", "rust"),
    ("sh", "bash"),
    ("zsh", "bash"),
    ("shell", "bash"),
    ("yml", "yaml"),
    ("js", "javascript"),
    ("mjs", "javascript"),
    ("cjs", "javascript"),
    ("jsx", "javascript"),
    ("ts", "typescript"),
    ("mts", "typescript"),
    ("py", "python"),
    ("golang", "go"),
    ("c++", "cpp"),
    ("cc", "cpp"),
    ("cxx", "cpp"),
    ("hpp", "cpp"),
    ("h", "cpp"),
    ("cs", "csharp"),
    ("kt", "kotlin"),
    ("kts", "kotlin"),
    ("tf", "terraform"),
    ("hcl", "terraform"),
    ("rb", "ruby"),
    ("htm", "html"),
    ("erl", "erlang"),
    ("hrl", "erlang"),
    ("hs", "haskell"),
    ("ex", "elixir"),
    ("exs", "elixir"),
    ("ml", "ocaml"),
    ("sbt", "scala"),
    ("sc", "scala"),
    ("rlang", "r"),
];

/// Resolves a fence info string or a file extension (with or without a
/// leading `.`) to a language, case-insensitively. A pure `&'static` table
/// lookup — no tree-sitter call, so it is safe on the UI thread. Returns
/// `None` for markdown and for anything unrecognised.
pub fn resolve(key: &str) -> Option<LangId> {
    let key = key.strip_prefix('.').unwrap_or(key).to_lowercase();
    let key = key.as_str();
    let name = LANGUAGES
        .iter()
        .find(|def| def.name == key)
        .map(|def| def.name)
        .or_else(|| {
            ALIASES
                .iter()
                .find(|(alias, _)| *alias == key)
                .map(|(_, name)| *name)
        })?;
    let id = LangId::from_name(name);
    assert_invariant!(id.is_some(), || format!(
        "language {name:?} known to rune-ts has no rune-syntax LangId"
    ));
    id
}
