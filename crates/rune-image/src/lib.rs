pub mod anim;
pub mod cellsize;
pub mod decode;
pub mod ids;
pub mod placeholder;
pub mod resize;
#[cfg(feature = "svg")]
pub mod svg;
pub mod transmit;

pub use anim::clamp_delay;
pub use cellsize::{CellFootprint, CellSize, DEFAULT_CELL_SIZE, PixelSize, fit_cells};
pub use decode::{
    Decoded, Format, ImageError, MAX_IMAGE_BYTES, decode_still, probe_dimensions, sniff_format,
};
pub use ids::{ImageId, alloc_id, frame_id_seed};
pub use placeholder::{PLACEHOLDER, diacritic};
pub use resize::{fit_box, resize};
#[cfg(feature = "svg")]
pub use svg::{SvgError, decode_svg};
pub use transmit::{
    Transmit, encode_delete, encode_delete_all, encode_transmit, encode_transmit_terminator,
    fit_and_encode,
};
