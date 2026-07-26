//! Command execution: movement/selection/scrolling (`nav`, WP6) and
//! editing/undo-redo (`edit`, WP7), dispatched from `app::handle_key`
//! against the `Command` the keymap resolver produces (plan Context,
//! "Keymap"). Structural port of `pkg/ui/components/textedit/commands_*.go`.

pub mod clipboard;
pub mod edit;
pub mod nav;
