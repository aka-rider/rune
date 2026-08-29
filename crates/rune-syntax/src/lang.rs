use rune_core::assert_invariant;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LangId(u8);

static NAMES: &[&str] = &[
    "rust",
    "json",
    "toml",
    "yaml",
    "bash",
    "python",
    "javascript",
    "go",
    "html",
    "css",
    "c",
    "cpp",
    "typescript",
    "tsx",
    "java",
    "csharp",
    "php",
    "ruby",
    "terraform",
    "sql",
    "kotlin",
    "swift",
    "erlang",
    "haskell",
    "elixir",
    "ocaml",
    "scala",
    "r",
    "lua",
    "make",
];

impl LangId {
    pub fn from_index(index: usize) -> Option<LangId> {
        (index < NAMES.len()).then_some(LangId(index as u8))
    }

    pub fn from_name(name: &str) -> Option<LangId> {
        NAMES
            .iter()
            .position(|n| *n == name)
            .map(|i| LangId(i as u8))
    }

    pub fn index(self) -> usize {
        self.0 as usize
    }

    pub fn name(self) -> &'static str {
        let name = NAMES.get(self.index()).copied();
        assert_invariant!(name.is_some(), || format!(
            "LangId index {} out of range",
            self.index()
        ));
        name.unwrap_or("")
    }

    pub fn count() -> usize {
        NAMES.len()
    }

    pub fn all() -> impl Iterator<Item = LangId> {
        (0..NAMES.len() as u8).map(LangId)
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
        assert_eq!(LangId::from_index(NAMES.len()), None);
        assert_eq!(LangId::from_index(0), Some(LangId(0)));
    }

    #[test]
    fn count_matches_the_name_table() {
        assert_eq!(LangId::count(), NAMES.len());
        assert_eq!(LangId::all().count(), NAMES.len());
    }
}
