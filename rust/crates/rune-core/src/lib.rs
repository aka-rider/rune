//! rune-core: UI-free kernel — buffer, coordinate spaces, cursor set, and
//! the in-memory undo journal. No terminal, no markdown parsing.

pub mod buffer;
pub mod coords;
pub mod cursor;
pub mod undo;
pub mod vfs;
