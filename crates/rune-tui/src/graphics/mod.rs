//! Terminal graphics capability (plan WP3): whether the Kitty graphics
//! protocol is usable and the measured pixel size of one terminal cell —
//! `App::graphics`'s type, populated once at startup (`runtime::bootstrap`)
//! and re-derived on every `Msg::Resize` (`runtime::apply`). Later work
//! packages (WP4+) build the image document and inline embeds on top of
//! this; nothing here decodes an image or renders a placeholder cell.

mod caps;
mod state;

pub use caps::{EnvSource, GraphicsCaps, ProcessEnv, detect};
pub use state::{ImageState, ImageStatus};
