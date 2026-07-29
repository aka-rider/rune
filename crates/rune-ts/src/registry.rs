//! The compiling half — parser and query construction. `registry()` is a
//! `LazyLock` that compiles all 22 languages' `Query`s the first time it is
//! touched. Call it only from a background command thread; UI-thread
//! callers that only need to identify a language use `lang::resolve`
//! instead, which never reaches this module.

use std::sync::LazyLock;

use tree_sitter::{Language, Parser, Query};

use crate::lang::LANGUAGES;

/// Every language that loaded and whose query compiled, plus every language
/// that didn't — populated once by `LanguageRegistry::new` and never
/// mutated afterward.
pub struct LanguageRegistry {
    loaded: Vec<(&'static str, (Language, Query))>,
    failures: Vec<(&'static str, String)>,
}

impl LanguageRegistry {
    /// Loads every language in [`LANGUAGES`]. A `Parser::set_language` or
    /// `Query::new` error is recorded as an entry in
    /// [`LanguageRegistry::failures`] and that language is skipped — never
    /// `unwrap`/`expect`/`panic` (surfaced, never silent).
    pub fn new() -> LanguageRegistry {
        let mut loaded = Vec::with_capacity(LANGUAGES.len());
        let mut failures = Vec::new();
        for def in LANGUAGES {
            let language = (def.language)();
            let mut parser = Parser::new();
            if let Err(err) = parser.set_language(&language) {
                failures.push((def.name, err.to_string()));
                continue;
            }
            let source = (def.highlights)();
            match Query::new(&language, &source) {
                Ok(query) => loaded.push((def.name, (language, query))),
                Err(err) => failures.push((def.name, err.to_string())),
            }
        }
        LanguageRegistry { loaded, failures }
    }

    /// The compiled `(Language, Query)` pair for a canonical language name,
    /// or `None` if that language failed to load — see
    /// [`LanguageRegistry::failures`].
    pub fn get(&self, name: &str) -> Option<&(Language, Query)> {
        self.loaded
            .iter()
            .find_map(|(n, pair)| (*n == name).then_some(pair))
    }

    /// Every language that failed to load or whose query failed to
    /// compile, paired with the error message that explains why.
    pub fn failures(&self) -> &[(&'static str, String)] {
        &self.failures
    }

    /// The canonical names of every language that loaded successfully.
    pub fn names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.loaded.iter().map(|(n, _)| *n)
    }
}

impl Default for LanguageRegistry {
    fn default() -> Self {
        LanguageRegistry::new()
    }
}

/// The lazily-compiled registry of all 22 languages. Touching this compiles
/// every grammar's query, so it must only ever be reached from a background
/// command — never from the UI thread.
pub fn registry() -> &'static LanguageRegistry {
    static REGISTRY: LazyLock<LanguageRegistry> = LazyLock::new(LanguageRegistry::new);
    &REGISTRY
}
