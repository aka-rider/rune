mod decode;
mod reconcile;
mod state;

pub use state::{EmbedSet, EmbedState};

pub(crate) use decode::{encode_embed_cmd, handle_embed_decoded, handle_embed_encoded};
pub(crate) use reconcile::{reload_embeds, sync_embeds};
