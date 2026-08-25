#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::HashSet;
use std::sync::OnceLock;

use rune_ts::lang::LANGUAGES;
use rune_ts::{capture_is_accounted_for, declared_capture_names, registry};

fn compiled_captures() -> &'static [(&'static str, String)] {
    static CAPTURES: OnceLock<Vec<(&'static str, String)>> = OnceLock::new();
    CAPTURES.get_or_init(|| {
        let mut captures = Vec::new();
        for def in LANGUAGES {
            let id = rune_ts::resolve(def.name).expect("every LangDef name resolves to a LangId");
            let compiled = registry().get(id);
            assert!(
                compiled.is_some(),
                "{}: highlight query failed to compile",
                def.name
            );
            if let Some((_language, query)) = compiled {
                for name in query.capture_names() {
                    captures.push((def.name, (*name).to_string()));
                }
            }
        }
        assert!(!captures.is_empty(), "no capture names were collected");
        captures
    })
}

fn all_compiled_capture_names() -> HashSet<&'static str> {
    compiled_captures()
        .iter()
        .map(|(_lang, name)| name.as_str())
        .collect()
}

#[test]
fn every_grammar_capture_resolves_or_is_deliberately_ignored() {
    for (lang, name) in compiled_captures() {
        assert!(
            capture_is_accounted_for(name),
            "{lang}: capture @{name} resolves to no scope, no alias, and is not deliberately ignored"
        );
    }
}

#[test]
fn every_declared_capture_name_is_still_used_by_some_grammar() {
    let used = all_compiled_capture_names();
    for declared in declared_capture_names() {
        assert!(
            used.contains(declared),
            "@{declared} is declared in rune-ts but no grammar's query emits it any more — delete the entry"
        );
    }
}
