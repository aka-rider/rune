//! Command execution: character/word movement/selection (`nav`),
//! line/document motion (`nav_line`), vertical/scroll motion and
//! viewport-only scrolling (`nav_scroll`), mouse gestures
//! (`mouse`), editing/undo-redo (`edit`), the shared buffer-mutation
//! chokepoint (`edit_core`), line-oriented editing (`edit_lines`
//! and `edit_lines_move`), multi-cursor management (`multi`),
//! the `⌃P` reading-view toggle (`reading`), and the read-only
//! "every motion key is a viewport command" policy (`reading_nav`),
//! dispatched from `app::handle_key`/`app::update` against the
//! `Command`/`Msg::Mouse` the keymap resolver/runtime produce.

pub mod case;
pub mod clipboard;
pub mod edit;
pub mod edit_core;
pub mod edit_lines;
pub mod edit_lines_move;
pub(crate) mod editor_exec;
pub mod language;
pub mod mouse;
pub(crate) mod mouse_hit;
pub mod multi;
pub mod nav;
pub mod nav_line;
pub mod nav_scroll;
pub mod reading;
pub mod reading_nav;
pub mod splitter;
#[cfg(test)]
pub(crate) mod test_support;
