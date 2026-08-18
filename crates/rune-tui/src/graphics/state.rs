//! `ImageState`: an image document's own state — file
//! identity, the allocated Kitty image id, what's known about its pixel and
//! cell geometry, the decoded pixels once decoded, and the
//! lifecycle status the info card / placeholder cells read. Lives on
//! `Document::image`, `None` for every non-image document.

use rune_image::{CellFootprint, ImageId, PixelSize};

/// An image document's decode/transmit lifecycle: a
/// still terminal-graphics FSM state, not yet the full one that will cover
/// inline embeds and animation. `Pending` is the state an image document
/// opens in — nothing decoded yet, so the info card shows a `"decoding…"`
/// reason line rather than an unexplained blank one (plan gotcha 9). `Live`
/// means a decode succeeded. `Failed` carries the reason the info card's own
/// reason line shows.
pub enum ImageStatus {
    Pending,
    Live {
        decoded: rune_image::decode::Decoded,
        cells: CellFootprint,
    },
    Failed(String),
}

/// One image document's state. `dims` is the pixel size —
/// populated at open time via `rune_image::probe_dimensions` (header-only,
/// no full decode) so the info card can show `WIDTHxHEIGHT` even before any
/// decode `Cmd` exists to populate `decoded`. `in_flight` carries the request
/// generation a currently-running async decode was spawned against —
/// `spawn_cmd` has no cancellation, so a stale reply must be recognisable
/// and dropped; `next_generation` mints a value strictly greater than any
/// generation this document has issued before — including one whose reply
/// never arrived — rather than deriving a value from `in_flight` (which is
/// `None` again once a decode finishes, and would otherwise let a later
/// spawn collide with a still-outstanding earlier one).
pub struct ImageState {
    pub path: std::path::PathBuf,
    pub bytes_len: u64,
    pub id: ImageId,
    pub dims: Option<PixelSize>,
    pub status: ImageStatus,
    pub in_flight: Option<crate::generation::Generation>,
    pub next_generation: crate::generation::GenCounter,
}
