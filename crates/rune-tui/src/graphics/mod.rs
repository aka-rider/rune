mod caps;
mod decode_cmd;
pub mod embed;
mod footprint;
mod resize_refit;
mod state;

pub use caps::{EnvSource, GraphicsCaps, ProcessEnv, detect};
pub use embed::{EmbedSet, EmbedState};
pub use state::{ImageState, ImageStatus};

pub enum Graphics {
    None,
    Image(ImageState),
    Embeds(EmbedSet),
}

pub(crate) use decode_cmd::{handle_image_decoded, handle_image_encoded, reload_image, schedule_image_decode};
pub(crate) use embed::{handle_embed_decoded, handle_embed_encoded, reload_embeds, sync_embeds};
pub(crate) use resize_refit::refit_on_resize;

pub(crate) fn redetect(app: &mut crate::app::App, guard: &mut crate::term::Guard) {
    app.graphics = detect(&ProcessEnv, guard.window_size());
    guard.set_kitty(app.graphics.kitty);
}
