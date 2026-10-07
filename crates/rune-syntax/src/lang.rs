use rune_core::assert_invariant;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LangId(u8);

struct LangSpec {
    name: &'static str,
    line_comments: &'static [&'static str],
}

static LANGS: &[LangSpec] = &[
    LangSpec {
        name: "rust",
        line_comments: &["///", "//!", "//"],
    },
    LangSpec {
        name: "json",
        line_comments: &[],
    },
    LangSpec {
        name: "toml",
        line_comments: &["#"],
    },
    LangSpec {
        name: "yaml",
        line_comments: &["#"],
    },
    LangSpec {
        name: "bash",
        line_comments: &["#"],
    },
    LangSpec {
        name: "python",
        line_comments: &["#"],
    },
    LangSpec {
        name: "javascript",
        line_comments: &["//"],
    },
    LangSpec {
        name: "go",
        line_comments: &["//"],
    },
    LangSpec {
        name: "html",
        line_comments: &[],
    },
    LangSpec {
        name: "css",
        line_comments: &[],
    },
    LangSpec {
        name: "c",
        line_comments: &["//"],
    },
    LangSpec {
        name: "cpp",
        line_comments: &["//"],
    },
    LangSpec {
        name: "typescript",
        line_comments: &["//"],
    },
    LangSpec {
        name: "tsx",
        line_comments: &["//"],
    },
    LangSpec {
        name: "java",
        line_comments: &["//"],
    },
    LangSpec {
        name: "csharp",
        line_comments: &["//"],
    },
    LangSpec {
        name: "php",
        line_comments: &["//", "#"],
    },
    LangSpec {
        name: "ruby",
        line_comments: &["#"],
    },
    LangSpec {
        name: "terraform",
        line_comments: &["//", "#"],
    },
    LangSpec {
        name: "sql",
        line_comments: &["--"],
    },
    LangSpec {
        name: "kotlin",
        line_comments: &["//"],
    },
    LangSpec {
        name: "swift",
        line_comments: &["//"],
    },
    LangSpec {
        name: "erlang",
        line_comments: &["%"],
    },
    LangSpec {
        name: "haskell",
        line_comments: &["--"],
    },
    LangSpec {
        name: "elixir",
        line_comments: &["#"],
    },
    LangSpec {
        name: "ocaml",
        line_comments: &[],
    },
    LangSpec {
        name: "scala",
        line_comments: &["//"],
    },
    LangSpec {
        name: "r",
        line_comments: &["#"],
    },
    LangSpec {
        name: "lua",
        line_comments: &["--"],
    },
    LangSpec {
        name: "make",
        line_comments: &["#"],
    },
    LangSpec {
        name: "postgres",
        line_comments: &["--"],
    },
    LangSpec {
        name: "plpgsql",
        line_comments: &["--"],
    },
];

impl LangId {
    pub fn from_index(index: usize) -> Option<LangId> {
        (index < LANGS.len()).then_some(LangId(index as u8))
    }

    pub fn from_name(name: &str) -> Option<LangId> {
        LANGS
            .iter()
            .position(|lang| lang.name == name)
            .map(|i| LangId(i as u8))
    }

    pub fn index(self) -> usize {
        self.0 as usize
    }

    pub fn name(self) -> &'static str {
        let name = LANGS.get(self.index()).map(|lang| lang.name);
        assert_invariant!(name.is_some(), || format!(
            "LangId index {} out of range",
            self.index()
        ));
        name.unwrap_or("")
    }

    pub fn line_comments(self) -> &'static [&'static str] {
        let spec = LANGS.get(self.index());
        assert_invariant!(spec.is_some(), || format!(
            "LangId index {} out of range",
            self.index()
        ));
        spec.map_or(&[], |lang| lang.line_comments)
    }

    pub fn count() -> usize {
        LANGS.len()
    }

    pub fn all() -> impl Iterator<Item = LangId> {
        (0..LANGS.len() as u8).map(LangId)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_name_round_trips_through_name() {
        for id in LangId::all() {
            assert_eq!(LangId::from_name(id.name()), Some(id));
        }
    }

    #[test]
    fn unknown_name_resolves_to_none() {
        assert_eq!(LangId::from_name("not-a-language"), None);
    }

    #[test]
    fn out_of_range_index_is_none() {
        assert_eq!(LangId::from_index(LANGS.len()), None);
        assert_eq!(LangId::from_index(0), Some(LangId(0)));
    }

    #[test]
    fn count_matches_the_name_table() {
        assert_eq!(LangId::count(), LANGS.len());
        assert_eq!(LangId::all().count(), LANGS.len());
    }

    #[test]
    fn line_comment_tokens_are_ordered_longest_first() {
        for id in LangId::all() {
            let tokens = id.line_comments();
            assert!(
                tokens
                    .iter()
                    .zip(tokens.iter().skip(1))
                    .all(|(longer, shorter)| longer.len() >= shorter.len()),
                "{} tokens not longest-first: {tokens:?}",
                id.name()
            );
        }
    }

    #[test]
    fn rust_tries_doc_comment_tokens_before_plain_slashes() {
        let rust = LangId::from_name("rust").map(LangId::line_comments);
        assert_eq!(rust, Some(&["///", "//!", "//"][..]));
    }

    #[test]
    fn python_has_hash_line_comments() {
        let python = LangId::from_name("python").map(LangId::line_comments);
        assert_eq!(python, Some(&["#"][..]));
    }

    #[test]
    fn json_has_no_line_comments() {
        let json = LangId::from_name("json").map(LangId::line_comments);
        assert_eq!(json, Some(&[][..]));
    }
}
