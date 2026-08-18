//! `ImageState` (plan WP4.S6): an image document's own state — file
//! identity, the allocated Kitty image id, what's known about its pixel and
//! cell geometry, the decoded pixels once WP5 decodes them, and the
//! lifecycle status the info card / placeholder cells read. Lives on
//! `Document::image`, `None` for every non-image document.

use rune_image::{CellFootprint, ImageId, PixelSize};

/// An image document's decode/transmit lifecycle (plan WP4.S6/WP4.S10): a
/// still terminal-graphics FSM state, not yet the full one WP9/WP10 add for
/// inline embeds and animation. `Pending` is the state an image document
/// opens in — nothing decoded yet, so the info card shows a `"decoding…"`
/// reason line rather than an unexplained blank one (plan gotcha 9). `Live`
/// means a decode succeeded (WP5 is what ever reaches it — WP4 never
/// decodes). `Failed` carries the reason the info card's own reason line
/// shows.
pub enum ImageStatus {
    Pending,
    Live {
        decoded: rune_image::decode::Decoded,
        cells: CellFootprint,
    },
    Failed(String),
}

/// One image document's state (plan WP4.S6). `dims` is the pixel size —
/// populated at open time via `rune_image::probe_dimensions` (header-only,
/// no full decode) so the info card can show `WIDTHxHEIGHT` even before any
/// decode `Cmd` exists to populate `decoded`. `in_flight` carries the request
/// generation a currently-running async decode was spawned against (WP5) —
/// `spawn_cmd` has no cancellation, so a stale reply must be recognisable
/// and dropped; `next_generation` is the last generation this document ever
/// issued, so a fresh spawn always mints a strictly greater value than any
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
    pub in_flight: Option<u64>,
    pub next_generation: u64,
}
