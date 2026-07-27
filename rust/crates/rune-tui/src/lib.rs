//! rune-tui: the Elm-style runtime, terminal lifecycle, keymap resolver, and
//! editor UI. Depends on rune-core and rune-md; owns the one place in the
//! workspace that talks to a real terminal (`term::Guard`).

pub mod app;
pub mod clipboard;
pub mod commands;
pub mod db;
pub mod document;
pub mod footer;
pub mod keymap;
pub mod listnav;
pub mod pane;
pub mod render;
pub mod runtime;
pub mod save;
pub mod styles;
pub mod term;
