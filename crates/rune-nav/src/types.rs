//! The navigation vocabulary's data shapes (500-line budget split of the
//! crate root): a `Ref`'s two kinds (`Use`/`Def`), what a `Use` points at
//! before resolution (`Target`), what it resolves to (`Destination`), and
//! the anchor/role types both sides share. No behavior lives here — see
//! the sibling `resolve`, `external`, and `anchor` modules for that.

use std::path::PathBuf;

use rune_syntax::element::ByteRange;

/// A single navigable reference found in a document: where it sits
/// (`site`) and what it is (`kind`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ref {
    pub site: ByteRange,
    pub kind: RefKind,
}

/// A reference is either a USE (an edge pointing somewhere) or a DEF (a
/// node something can point at).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RefKind {
    Use { role: UseRole, target: Target },
    Def { role: DefRole, name: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UseRole {
    Link,
    Embed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DefRole {
    Heading(u8),
}

/// What a `Use` points at, before resolution against the filesystem.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Url(String),
    Path {
        path: String,
        anchor: Option<Anchor>,
    },
    Name {
        name: String,
        anchor: Option<Anchor>,
    },
    SameDoc(Anchor),
}

/// What kind of def an `Anchor::Named` matches against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnchorRole {
    Heading,
}

/// An anchor is either name-based (matched against a `Def`'s name via
/// `anchor_matches`) or positional (a source line number, independent of
/// any def). The two shapes cannot be fused into one `name: String` field
/// without a positional anchor losing its number — a `Named` anchor's
/// `role` says which kind of def it searches for; a `Line` anchor never
/// touches a document's defs at all.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Anchor {
    Named {
        role: AnchorRole,
        name: String,
    },
    /// A 1-based source line number, matching the user-facing `Ln`
    /// convention (footer readout, editor tools' `#L<n>` links).
    Line(u32),
}

/// Where a `Target` actually resolves to. `Unresolved` is deliberately a
/// real state, not an error: an unresolvable link is still a graph edge the
/// future vault graph must draw, and the UI reports it rather than hiding
/// it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Destination {
    Url(String),
    Location {
        path: PathBuf,
        anchor: Option<Anchor>,
    },
    Unresolved,
}
