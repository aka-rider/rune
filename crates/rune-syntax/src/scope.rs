//! The open scope namespace (WP4) that replaces the closed 23-variant
//! `StyleId` enum: a dotted-name -> dense `ScopeId` table, resolved by
//! **longest-dotted-prefix** — strip the name after its last `.` and retry
//! until a hit or no dots remain. Helix (`rfind('.')` loop), Zed (`BTreeMap`
//! range search) and Neovim (`@comment.documentation` -> `@comment`)
//! converged on this rule independently: it lets an unknown capture from an
//! updated grammar degrade to its parent scope instead of vanishing.
//!
//! `rune-syntax` owns this table; a theme (`rune-tui`) only ever maps a
//! resolved `ScopeId` to a rendered `Style` — it never registers or
//! resolves a name itself.

use std::collections::HashMap;

/// A dense, table-relative scope handle — `SyntaxSpan`'s tag after WP4
/// (replaces the closed `StyleId` enum). Meaningless outside the
/// `ScopeTable` that minted it: two tables built independently may assign
/// the same numeric id to different names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ScopeId(pub u16);

/// Dotted-name -> `ScopeId` registry with longest-dotted-prefix resolution.
/// Open (any producer can `register` a name it needs at configure time),
/// unlike the closed enum it replaces — but every producer today
/// (`rune-md`'s comrak-driven emitter, `rune-ts`'s tree-sitter one) builds
/// its table from the one shared [`scope_table`] constructor below rather
/// than calling `register` with names of its own choosing, so the
/// vocabulary is open in principle but closed in practice: a producer
/// whose capture names fall outside [`MARKDOWN_SCOPES`]/[`CODE_SCOPES`]
/// has no way to extend the table it's handed, and `resolve` silently
/// drops such a capture down to `None` rather than registering it.
#[derive(Debug, Default)]
pub struct ScopeTable {
    names: Vec<String>,
    index: HashMap<String, ScopeId>,
}

impl ScopeTable {
    pub fn new() -> ScopeTable {
        ScopeTable::default()
    }

    /// Registers `name`, returning its `ScopeId`. Idempotent: registering
    /// the same name twice returns the same id rather than minting a
    /// duplicate — callers never need to check `resolve` first.
    pub fn register(&mut self, name: &str) -> ScopeId {
        if let Some(&id) = self.index.get(name) {
            return id;
        }
        let id = ScopeId(self.names.len() as u16);
        self.names.push(name.to_string());
        self.index.insert(name.to_string(), id);
        id
    }

    /// Resolves `name` to its `ScopeId`, falling back to progressively
    /// shorter dotted prefixes when the exact name isn't registered — the
    /// rule this module's docs describe. `"markup.heading.marker"` with
    /// only `"markup.heading"` registered resolves to that parent's id;
    /// `None` only when no prefix, down to the bare first segment, is
    /// registered either.
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

    /// The registered name for `id`, or `None` if `id` was never minted by
    /// this table (a stale id from a different table).
    pub fn name(&self, id: ScopeId) -> Option<&str> {
        self.names.get(id.0 as usize).map(String::as_str)
    }

    /// Number of distinct scopes registered — the length a theme's
    /// `scopes: Vec<Style>` must have to cover every `ScopeId` this table
    /// can hand out.
    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// Every `(ScopeId, name)` pair in registration order — the one walk a
    /// theme builder needs to size and fill its `scopes` vector so its
    /// index space agrees with this table's.
    pub fn iter(&self) -> impl Iterator<Item = (ScopeId, &str)> {
        self.names
            .iter()
            .enumerate()
            .map(|(i, n)| (ScopeId(i as u16), n.as_str()))
    }
}

/// The canonical markdown scope vocabulary (WP4.S2's `StyleId` -> scope
/// mapping) in the order that fixes each name's `ScopeId`. `rune-md`'s
/// emitter resolves against [`scope_table`] built from this exact list,
/// and `rune-tui`'s `Theme` walks the SAME table when it builds its
/// `scopes: Vec<Style>` — one shared table is what keeps both sides
/// agreeing on which id means which name, without either depending on the
/// other.
pub const MARKDOWN_SCOPES: &[&str] = &[
    "text",
    "markup.heading.1",
    "markup.heading.2",
    "markup.heading.3",
    "markup.heading.4",
    "markup.heading.5",
    "markup.heading.6",
    "markup.strong",
    "markup.italic",
    "markup.strikethrough",
    "markup.raw.inline",
    "markup.raw.block",
    "markup.link",
    "markup.quote",
    "markup.list",
    "markup.list.checked",
    "markup.table",
    "markup.table.header",
    "markup.table.separator",
    "markup.table.border",
    "punctuation.special",
    "comment",
    "markup.quote.marker",
];

/// The canonical code-token scope vocabulary a tree-sitter producer resolves
/// its grammar captures against, appended after [`MARKDOWN_SCOPES`] so
/// markdown ids stay fixed at `0..=22`. `"comment"` is deliberately absent —
/// it is already registered by `MARKDOWN_SCOPES` and a code capture landing
/// on `@comment` resolves to that shared id instead of a duplicate.
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

/// Scopes registered AFTER [`CODE_SCOPES`] rather than folded into either
/// earlier table (WP7): appending here keeps [`MARKDOWN_SCOPES`] fixed at
/// `0..=22` and every [`CODE_SCOPES`] id exactly where it already was —
/// inserting a new name into either earlier table would renumber every id
/// that follows it, since ids are assigned by registration order. `rune-md`'s
/// image emission (`emit::style::image_scope`) is the one resolver today.
pub const EXTENDED_SCOPES: &[&str] = &["markup.image"];

/// Builds a fresh `ScopeTable` pre-registered with [`MARKDOWN_SCOPES`], then
/// [`CODE_SCOPES`], then [`EXTENDED_SCOPES`], in that order. Exposed as a
/// constructor (rather than a lazily-initialized static) so both the
/// emitter and a theme built in a test can each own their own instance and
/// still agree on ids, as long as they're both built from this same
/// function. The one shared constructor every producer (comrak-driven
/// `rune-md`, tree-sitter-driven `rune-ts`) and the theme (`rune-tui`)
/// build from independently.
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
        // "markup.heading.marker" is never registered directly — an
        // unknown, more-specific capture from an updated grammar — but it
        // must resolve to its registered parent rather than vanish.
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

    /// WP7: appending `EXTENDED_SCOPES` after `CODE_SCOPES` must not
    /// renumber any code scope — pins the first `CODE_SCOPES` entry's id at
    /// exactly `MARKDOWN_SCOPES.len()`, unchanged from before this package.
    #[test]
    fn extended_scopes_do_not_renumber_code_scopes() {
        let table = scope_table();
        let first_code = CODE_SCOPES.first().copied().unwrap_or_default();
        assert_eq!(
            table.resolve(first_code),
            Some(ScopeId(MARKDOWN_SCOPES.len() as u16))
        );
    }

    /// `markup.image` lands strictly after every `CODE_SCOPES` id, proving
    /// it was appended rather than inserted earlier in the table.
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
        // A half-done append would leave "markup.quote.marker" unregistered,
        // in which case longest-dotted-prefix resolution would silently
        // fall back to "markup.quote" instead of failing loudly — this
        // guards that the append actually registered the more specific name.
        assert_ne!(
            table.resolve("markup.quote.marker"),
            table.resolve("markup.quote")
        );
    }
}
