//! The prefix index (plan WP6.S4): built from ONE binding table at a time.
//! A table is a "binding set" (`crate::global::GLOBAL_BINDINGS`,
//! `editor_bindings::EDITOR_BINDINGS`, `vim::VIM_BINDINGS`, ...) and
//! validation never crosses that boundary — two DIFFERENT sets may legally
//! share a prefix relationship neither could tolerate against itself,
//! because only one set is ever live at once (the vim set deliberately
//! reuses `h`/`j`/`k`/`l`/`i`, keys the default editor set also binds).
//!
//! `validate.rs` holds the collision-checking side (`validate`,
//! `BindingConflict`, `PrefixCollision`); `state.rs` holds the tri-state
//! resolver (`Resolution`, WP6.S5) and the chord-in-progress state
//! (`KeymapState`) a focused component threads a keystroke through, plus
//! the out-of-band single-key hook (`on_next_key`, WP6.S6). Split across
//! the two so this hub stays under the §1.6 500-line budget; every item is
//! re-exported here so no `keymap::index::` import path downstream changed.
//! This is a SEPARATE mechanism from the held-space leader
//! (`global::LEADER_BINDINGS`/`keystate::SpaceProbe`), which resolves a
//! single key confirmed by a physical hardware probe rather than a typed
//! multi-key sequence — the two do not share state and `app::handle_key`
//! consults the leader stage first (§3.4's matching order).

mod state;
mod validate;

pub use state::{KeymapState, NextKeyFn, Resolution, resolve, resolve_stateful};
pub use validate::{BindingConflict, PrefixCollision, validate};
