use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ScopeId(pub u16);

#[derive(Debug, Default)]
pub struct ScopeTable {
    names: Vec<String>,
    index: HashMap<String, ScopeId>,
}

impl ScopeTable {
    pub fn new() -> ScopeTable {
        ScopeTable::default()
    }

    fn register(&mut self, name: &str) -> ScopeId {
        if let Some(&id) = self.index.get(name) {
            return id;
        }
        let id = ScopeId(self.names.len() as u16);
        self.names.push(name.to_string());
        self.index.insert(name.to_string(), id);
        id
    }

    pub fn resolve(&self, name: &str) -> Option<ScopeId> {
        let mut candidate = name;
        loop {
            if let Some(&id) = self.index.get(candidate) {
                return Some(id);
            }
            let pos = candidate.rfind('.')?;
            candidate = &candidate[..pos];
        }
    }

    pub fn name(&self, id: ScopeId) -> Option<&str> {
        self.names.get(id.0 as usize).map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (ScopeId, &str)> {
        self.names
            .iter()
            .enumerate()
            .map(|(i, n)| (ScopeId(i as u16), n.as_str()))
    }
}

macro_rules! markdown_scopes {
    ($($variant:ident => $name:literal),+ $(,)?) => {
        pub const MARKDOWN_SCOPES: &[&str] = &[$($name),+];

        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum MarkdownScope {
            $($variant),+
        }

        impl MarkdownScope {
            pub const ALL: &'static [MarkdownScope] = &[$(MarkdownScope::$variant),+];

            pub const fn name(self) -> &'static str {
                match self {
                    $(MarkdownScope::$variant => $name),+
                }
            }
        }

        impl From<MarkdownScope> for ScopeId {
            fn from(scope: MarkdownScope) -> ScopeId {
                ScopeId(scope as u16)
            }
        }

        impl TryFrom<ScopeId> for MarkdownScope {
            type Error = ();

            fn try_from(id: ScopeId) -> Result<MarkdownScope, ()> {
                match id.0 {
                    $(x if x == MarkdownScope::$variant as u16 => Ok(MarkdownScope::$variant),)+
                    _ => Err(()),
                }
            }
        }
    };
}

markdown_scopes! {
    Text => "text",
    Heading1 => "markup.heading.1",
    Heading2 => "markup.heading.2",
    Heading3 => "markup.heading.3",
    Heading4 => "markup.heading.4",
    Heading5 => "markup.heading.5",
    Heading6 => "markup.heading.6",
    Strong => "markup.strong",
    Italic => "markup.italic",
    Strikethrough => "markup.strikethrough",
    RawInline => "markup.raw.inline",
    RawBlock => "markup.raw.block",
    Link => "markup.link",
    Quote => "markup.quote",
    List => "markup.list",
    ListChecked => "markup.list.checked",
    Table => "markup.table",
    TableHeader => "markup.table.header",
    TableSeparator => "markup.table.separator",
    TableBorder => "markup.table.border",
    PunctuationSpecial => "punctuation.special",
    Comment => "comment",
    QuoteMarker => "markup.quote.marker",
}

pub const CODE_SCOPES: &[&str] = &[
    "keyword",
    "function",
    "function.method",
    "type",
    "type.builtin",
    "constructor",
    "variable",
    "variable.parameter",
    "variable.member",
    "property",
    "constant",
    "constant.builtin",
    "string",
    "string.escape",
    "string.regexp",
    "number",
    "boolean",
    "operator",
    "punctuation",
    "punctuation.bracket",
    "punctuation.delimiter",
    "attribute",
    "label",
    "tag",
];

pub const EXTENDED_SCOPES: &[&str] = &["markup.image"];

pub const IMAGE_SCOPE_ID: ScopeId = ScopeId((MARKDOWN_SCOPES.len() + CODE_SCOPES.len()) as u16);

pub fn scope_table() -> ScopeTable {
    let mut table = ScopeTable::new();
    for name in MARKDOWN_SCOPES {
        table.register(name);
    }
    for name in CODE_SCOPES {
        table.register(name);
    }
    for name in EXTENDED_SCOPES {
        table.register(name);
    }
    table
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_is_idempotent() {
        let mut table = ScopeTable::new();
        let a = table.register("markup.strong");
        let b = table.register("markup.strong");
        assert_eq!(a, b);
        assert_eq!(table.len(), 1);
    }

    #[test]
    fn resolve_exact_match() {
        let table = scope_table();
        assert!(table.resolve("markup.heading.1").is_some());
    }

    #[test]
    fn resolve_falls_back_to_the_longest_registered_dotted_prefix() {
        let mut table = ScopeTable::new();
        let parent = table.register("markup.heading");
        assert_eq!(table.resolve("markup.heading.marker"), Some(parent));
    }

    #[test]
    fn resolve_returns_none_when_no_prefix_is_registered() {
        let table = ScopeTable::new();
        assert_eq!(table.resolve("keyword.control.conditional"), None);
    }

    #[test]
    fn scope_table_registers_every_canonical_scope() {
        let table = scope_table();
        assert_eq!(
            table.len(),
            MARKDOWN_SCOPES.len() + CODE_SCOPES.len() + EXTENDED_SCOPES.len()
        );
        for name in MARKDOWN_SCOPES {
            assert!(table.resolve(name).is_some(), "missing scope: {name}");
        }
        for name in CODE_SCOPES {
            assert!(table.resolve(name).is_some(), "missing scope: {name}");
        }
        for name in EXTENDED_SCOPES {
            assert!(table.resolve(name).is_some(), "missing scope: {name}");
        }
    }

    #[test]
    fn extended_scopes_do_not_renumber_code_scopes() {
        let table = scope_table();
        let first_code = CODE_SCOPES.first().copied().unwrap_or_default();
        assert_eq!(
            table.resolve(first_code),
            Some(ScopeId(MARKDOWN_SCOPES.len() as u16))
        );
    }

    #[test]
    fn markup_image_scope_is_registered_after_code_scopes() {
        let table = scope_table();
        let image_id = table.resolve("markup.image").unwrap_or(ScopeId(0));
        let last_code = table
            .resolve(CODE_SCOPES.last().copied().unwrap_or_default())
            .unwrap_or(ScopeId(0));
        assert!(image_id.0 > last_code.0);
    }

    #[test]
    fn code_capture_resolves_by_longest_dotted_prefix() {
        assert_eq!(
            scope_table().resolve("keyword.control.return"),
            scope_table().resolve("keyword")
        );
        assert_eq!(
            scope_table().resolve("variable.builtin"),
            scope_table().resolve("variable")
        );
    }

    #[test]
    fn markup_heading_scope_still_resolves() {
        assert!(scope_table().resolve("markup.heading.1").is_some());
    }

    #[test]
    fn markdown_scopes_still_start_at_id_zero() {
        let table = scope_table();
        let first = MARKDOWN_SCOPES.first().copied().unwrap_or_default();
        assert_eq!(table.resolve(first), Some(ScopeId(0)));
    }

    #[test]
    fn quote_marker_scope_is_registered_and_distinct_from_its_prefix_fallback() {
        let table = scope_table();
        assert_ne!(
            table.resolve("markup.quote.marker"),
            table.resolve("markup.quote")
        );
    }

    #[test]
    fn markdown_scope_variants_match_the_registration_list_by_position() {
        assert_eq!(MarkdownScope::ALL.len(), MARKDOWN_SCOPES.len());
        for (i, (scope, name)) in MarkdownScope::ALL.iter().zip(MARKDOWN_SCOPES).enumerate() {
            assert_eq!(scope.name(), *name);
            assert_eq!(ScopeId::from(*scope), ScopeId(i as u16));
        }
    }

    #[test]
    fn markdown_scope_round_trips_through_scope_id() {
        for scope in MarkdownScope::ALL {
            let id = ScopeId::from(*scope);
            assert_eq!(MarkdownScope::try_from(id), Ok(*scope));
        }
    }

    #[test]
    fn markdown_scope_agrees_with_the_shared_table() {
        let table = scope_table();
        for scope in MarkdownScope::ALL {
            assert_eq!(table.resolve(scope.name()), Some(ScopeId::from(*scope)));
        }
    }

    #[test]
    fn image_scope_id_matches_table_resolution() {
        let table = scope_table();
        assert_eq!(table.resolve("markup.image"), Some(IMAGE_SCOPE_ID));
    }
}
