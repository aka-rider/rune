//! Command execution: movement/selection/scrolling (`nav`, WP6). WP7 adds
//! `edit` (editing/undo-redo) alongside it. Dispatched from
//! `app::handle_key` against the `Command` the keymap resolver produces
//! (plan Context, "Keymap"). Structural port of
//! `pkg/ui/components/textedit/commands_*.go`.

pub mod nav;
