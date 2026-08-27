//! The compiling half — parser and query construction. Grammar queries are
//! compiled per-language on first request via [`LanguageRegistry::get`], not
//! eagerly. The [`registry()`] function provides a lazily-initialized shared
//! instance that may be touched from any thread; whichever thread first
//! requests a language pays that language's one-time compile.

use std::sync::{LazyLock, OnceLock};

use rune_core::assert_invariant;
use rune_syntax::LangId;
use tree_sitter::{Language, Parser, Query};

use crate::lang::{LANGUAGES, LangDef};

/// Compiled language and its highlight query, or a compilation error message.
type LanguageEntry = OnceLock<Result<(Language, Query), String>>;

/// A lazily-compiling registry of tree-sitter languages and their highlight
/// queries. Each language is compiled only when first requested via [`get`],
/// not when the registry is constructed. Errors are recorded and surfaced
/// through [`failures`], never panicking. Indexed by [`LangId`]: entry `i`
/// holds the language whose id's index is `i`, or `None` when no `LangDef`
/// in [`LANGUAGES`] names that id's language.
pub struct LanguageRegistry {
    entries: Vec<Option<(&'static LangDef, LanguageEntry)>>,
}

impl LanguageRegistry {
    /// Creates a new registry without compiling any languages. Compilation
    /// occurs on the first call to [`get`] for each language.
    pub fn new() -> LanguageRegistry {
        let entries = LangId::all()
            .map(|id| {
                let def = LANGUAGES.iter().find(|def| def.name == id.name());
                assert_invariant!(def.is_some(), || format!(
                    "no rune-ts LangDef for rune-syntax language {:?}",
                    id.name()
                ));
                def.map(|def| (def, OnceLock::new()))
            })
            .collect();
        LanguageRegistry { entries }
    }

    /// The compiled `(Language, Query)` pair for a language. Compilation
    /// happens on first call for each language. If that language failed to
    /// load or compile, returns `None` — see [`failures`] for the error
    /// message.
    pub fn get(&self, id: LangId) -> Option<&(Language, Query)> {
        let (def, slot) = self.entries.get(id.index())?.as_ref()?;
        let result = slot.get_or_init(|| {
            let language = (def.language)();
            let mut parser = Parser::new();
            if let Err(err) = parser.set_language(&language) {
                return Err(err.to_string());
            }
            let source = (def.highlights)();
            Query::new(&language, &source)
                .map(|query| (language, query))
                .map_err(|err| err.to_string())
        });
        result.as_ref().ok()
    }

    /// Every language that failed to load or whose query failed to compile,
    /// paired with the error message. This iterates only over entries that
    /// have been requested (and thus initialized); unrequested languages do
    /// not appear here.
    pub fn failures(&self) -> Vec<(&'static str, String)> {
        self.entries
            .iter()
            .flatten()
            .filter_map(|(def, slot)| {
                slot.get()
                    .and_then(|r| r.as_ref().err())
                    .map(|err| (def.name, err.clone()))
            })
            .collect()
    }

    /// The canonical names of every language in the registry, compiled or
    /// not. All 22 languages are always listed, regardless of whether they
    /// have been requested or have compiled successfully.
    pub fn names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.entries.iter().flatten().map(|(def, _)| def.name)
    }

    /// The count of languages that have been compiled successfully. A language
    /// is counted only after it has been requested via [`get`] and compiled
    /// without error.
    pub fn compiled_count(&self) -> usize {
        self.entries
            .iter()
            .flatten()
            .filter(|(_, slot)| slot.get().is_some_and(std::result::Result::is_ok))
            .count()
    }
}

impl Default for LanguageRegistry {
    fn default() -> Self {
        LanguageRegistry::new()
    }
}

/// A lazily-initialized shared registry of all 22 supported languages. Each
/// language's `Query` is compiled on its first request, not when this static
/// is touched.
pub fn registry() -> &'static LanguageRegistry {
    static REGISTRY: LazyLock<LanguageRegistry> = LazyLock::new(LanguageRegistry::new);
    &REGISTRY
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn failures_reports_a_failed_language() {
        let def: &'static LangDef = &LANGUAGES[0];
        let slot: LanguageEntry = OnceLock::new();
        slot.set(Err("synthetic failure".to_string()))
            .expect("freshly created OnceLock accepts the first set");
        let registry = LanguageRegistry {
            entries: vec![Some((def, slot))],
        };
        assert_eq!(
            registry.failures(),
            vec![(def.name, "synthetic failure".to_string())]
        );
    }
}
