//! Terminal graphics capability (plan WP3): whether the Kitty graphics
//! protocol is usable and the measured pixel size of one terminal cell —
//! `App::graphics`'s type, populated once at startup (`runtime::bootstrap`)
//! and re-derived on every `Msg::Resize` (`runtime::apply`). WP5 adds the
//! image document's decode/re-fit pipeline on top of this: `footprint`
//! (pure fit-to-width math), `decode_cmd` (the decode `Cmd`, its
//! scheduling chokepoint, and the reply handler), and `resize_refit` (the
//! `Msg::Resize`-driven re-fit/retransmit). WP9 adds `embed`, the same
//! shape again but for the SEVERAL inline embeds a markdown document may
//! hold at once, rather than the one image a whole image document is.

mod caps;
mod decode_cmd;
pub mod embed;
mod footprint;
mod resize_refit;
mod state;

pub use caps::{EnvSource, GraphicsCaps, ProcessEnv, detect};
pub use embed::{EmbedSet, EmbedState};
pub use state::{ImageState, ImageStatus};

// Crate-internal: `dispatch.rs` and `app.rs` call these by this
// `graphics::` path, but no external consumer of this crate (`rune-cli`,
// an integration test) has any business spawning a decode or re-fitting a
// footprint directly.
pub(crate) use decode_cmd::{handle_image_decoded, schedule_image_decode};
pub(crate) use embed::{handle_embed_decoded, sync_embeds};
pub(crate) use resize_refit::refit_on_resize;
