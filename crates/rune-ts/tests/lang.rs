//! Compile-free `lang::resolve` coverage that needs no registry — the
//! registry (query/parser compilation) is added by a later work package.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use rune_syntax::LangId;
use rune_ts::lang::{self, ALIASES, LANGUAGES};

#[test]
fn resolves_every_alias() {
    for (alias, name) in ALIASES {
        assert!(
            LANGUAGES.iter().any(|def| def.name == *name),
            "alias {alias:?} points at unknown language {name:?}"
        );
        assert_eq!(
            lang::resolve(alias).map(LangId::name),
            Some(*name),
            "alias {alias:?} did not resolve to {name:?}"
        );
    }
    assert_eq!(lang::resolve("md"), None);
}

#[test]
fn resolve_touches_no_grammar() {
    assert_eq!(lang::resolve("rs").map(LangId::name), Some("rust"));
}
