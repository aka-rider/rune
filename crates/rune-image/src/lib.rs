//! rune-image: pixel decode/resize/encode for inline terminal image
//! rendering (Kitty graphics protocol). Terminal-free — this crate must
//! never depend on `ratatui`, `termina`, or any `rune-tui` type, mirroring
//! the equivalent rule stated in `rune-md`. It deals only in image bytes,
//! pixels, and escape-sequence strings.

pub mod anim;
pub mod cellsize;
pub mod decode;
pub mod ids;
pub mod placeholder;
pub mod resize;
pub mod transmit;

pub use anim::clamp_delay;
pub use cellsize::{CellSize, DEFAULT_CELL_SIZE, fit_cells};
pub use decode::{Decoded, Format, decode_still, probe_dimensions, sniff_format};
pub use ids::{alloc_id, frame_id_seed};
pub use placeholder::{PLACEHOLDER, diacritic};
pub use resize::{fit_box, resize};
pub use transmit::{encode_delete, encode_delete_all, encode_transmit};
