pub mod decor;
pub mod element;
pub mod kind;
pub mod lang;
pub mod scope;
pub mod syntax;
pub mod wrap;

pub use decor::{DecorPiece, LineDecor};
pub use kind::DocumentKind;
pub use lang::LangId;
pub use scope::{ScopeId, ScopeTable};
pub use syntax::{CellMap, SyntaxLine, SyntaxSnapshot, SyntaxSpan, merge_overlapping};
