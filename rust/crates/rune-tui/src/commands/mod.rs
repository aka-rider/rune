//! Command execution: movement/selection (`nav`, WP6), vertical/scroll
//! motion and viewport-only scrolling (`nav_scroll`, WP7), mouse gestures
//! (`mouse`, WP7), and editing/undo-redo (`edit`), dispatched from
//! `app::handle_key`/`app::update` against the `Command`/`Msg::Mouse` the
//! keymap resolver/runtime produce (plan Context, "Keymap"). Structural
//! port of `pkg/ui/components/textedit/commands_*.go`.

pub mod clipboard;
pub mod edit;
pub mod mouse;
pub mod nav;
pub mod nav_scroll;
