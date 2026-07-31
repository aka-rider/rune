//! Command execution: character/word movement/selection (`nav`, WP6),
//! line/document motion (`nav_line`), vertical/scroll motion and
//! viewport-only scrolling (`nav_scroll`, WP7), mouse gestures
//! (`mouse`, WP7), editing/undo-redo (`edit`), the shared buffer-mutation
//! chokepoint (`edit_core`, WP9.S6), line-oriented editing (`edit_lines`
//! and `edit_lines_move`, WP9), and multi-cursor management (`multi`,
//! WP9), dispatched from
//! `app::handle_key`/`app::update` against the `Command`/`Msg::Mouse` the
//! keymap resolver/runtime produce (plan Context, "Keymap"). Structural
//! port of Go's textedit command family.

pub mod clipboard;
pub mod edit;
pub mod edit_core;
pub mod edit_lines;
pub mod edit_lines_move;
pub mod mouse;
mod mouse_hit;
pub mod multi;
pub mod nav;
pub mod nav_line;
pub mod nav_scroll;
pub mod splitter;
