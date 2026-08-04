//! The prefix index: built from ONE binding table at a time. A table is a
//! "binding set" (`crate::global::GLOBAL_BINDINGS`,
//! `editor_bindings::EDITOR_BINDINGS`, `vim::VIM_BINDINGS`, ...) and
//! validation never crosses that boundary — two DIFFERENT sets may legally
//! share a prefix relationship neither could tolerate against itself,
//! because only one set is ever live at once (the vim set deliberately
//! reuses `h`/`j`/`k`/`l`/`i`, keys the default editor set also binds).
//!
//! `validate.rs` holds the collision-checking side (`validate`,
//! `BindingConflict`, `PrefixCollision`) — the only member of this module,
//! re-exported here so no `keymap::index::` import path downstream changed.

mod validate;

pub use validate::{BindingConflict, PrefixCollision, validate};
