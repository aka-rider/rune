//! rune-tui: the Elm-style runtime, terminal lifecycle, keymap resolver, and
//! editor UI. Depends on rune-core and rune-md; owns the one place in the
//! workspace that talks to a real terminal (`term::Guard`).

pub mod app;
pub mod banner;
pub mod binding;
pub mod breadcrumb;
pub mod clipboard;
pub mod commands;
pub mod db;
mod dispatch;
pub mod document;
pub mod explorer;
pub mod focus;
pub mod footer;
pub mod global;
pub mod help;
pub mod highlight;
pub mod keymap;
pub mod keystate;
pub mod layout;
pub mod listnav;
pub mod navigate;
pub mod opentabs;
pub mod pane;
pub mod pointer;
pub mod rename;
pub mod render;
pub mod row_meta;
pub mod runtime;
pub mod save;
pub mod split;
pub mod term;
#[cfg(any(test, feature = "testgrid"))]
pub mod testgrid;
pub mod theme;
pub mod title;
pub mod when;
pub mod workspace;
pub mod workspaceroot;
