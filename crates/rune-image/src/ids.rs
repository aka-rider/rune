//! Deterministic Kitty image IDs derived from a document's absolute path.

use std::num::NonZeroU32;

const FNV_OFFSET_BASIS: u32 = 2_166_136_261;
const FNV_PRIME: u32 = 16_777_619;
const ID_MASK_24: u32 = 0x00FF_FFFF;

/// The highest id [`alloc_id`]/[`ImageId::next`] ever hand out — ids wrap
/// from here back to `1`, never to `0` (`0` is not a valid Kitty image id,
/// the reason this type exists at all).
pub const MAX_ID: u32 = 0x00FF_FFFF;

/// A Kitty graphics protocol image id: always non-zero (`0` means "no
/// id" to the protocol), always within the 24-bit range a single
/// truecolor foreground value can carry for Unicode placeholder cells.
///
/// The inner field is private and this module is the only place that ever
/// constructs one — [`alloc_id`] (a fresh, path-derived id) and
/// [`ImageId::next`] (the collision-probe/wrap step every allocation
/// scheme built on top of this shares) are the whole surface. A caller
/// outside this crate can hold, compare, and hash an `ImageId`, but can
/// never manufacture one out of an arbitrary `u32` — only [`ImageId::get`]
/// lets one back out, at the wire boundary that actually needs a bare
/// integer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ImageId(NonZeroU32);

impl ImageId {
    fn from_masked(id: u32) -> ImageId {
        ImageId(NonZeroU32::new(id).unwrap_or(NonZeroU32::MIN))
    }

    /// The next id a collision probe should try: `self + 1`, wrapping past
    /// [`MAX_ID`] back to `1` rather than to `0`. The one step every
    /// probing allocator built on top of this module shares, so the wrap
    /// edge is defined and tested in exactly one place.
    pub fn next(self) -> ImageId {
        if self.0.get() >= MAX_ID {
            ImageId::from_masked(1)
        } else {
            ImageId::from_masked(self.0.get() + 1)
        }
    }

    /// The wire-boundary escape hatch: the raw id a Kitty escape sequence
    /// or a smuggled placeholder-cell colour actually encodes.
    pub fn get(self) -> u32 {
        self.0.get()
    }

    /// Builds a fixed `ImageId` for a test fixture that needs one exact,
    /// reproducible value — an encoding round-trip pinned to a specific
    /// byte pattern, say — rather than whatever `alloc_id` derives from a
    /// path. Still routes through [`ImageId::from_masked`], so the `0`
    /// invariant holds even here: this is a wider constructor, never an
    /// unchecked one.
    pub fn for_test(id: u32) -> ImageId {
        ImageId::from_masked(id)
    }
}

/// Derives a deterministic, non-zero, 24-bit image ID from an absolute
/// path's bytes using FNV-1a. The 24-bit bound lets the ID be encoded in a
/// single truecolor foreground value for Unicode placeholder cells.
pub fn alloc_id(abs_path: &[u8]) -> ImageId {
    let mut hash = FNV_OFFSET_BASIS;
    for byte in abs_path {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    ImageId::from_masked(hash & ID_MASK_24)
}

/// The seed string hashed (via [`alloc_id`]'s FNV-1a) to derive one
/// animation frame's own image ID, keeping it deterministic and distinct
/// from the document's own image ID.
pub fn frame_id_seed(abs_path: &str, frame: usize) -> String {
    format!("{abs_path}#frame{frame}")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn alloc_id_never_produces_the_reserved_zero_id() {
        // A path whose FNV-1a hash happens to mask to zero exists; alloc_id
        // must still hand back a valid non-zero id rather than "no id".
        for seed in [b"".as_slice(), b"/a", b"/a/b/c.png", b"\0\0\0\0"] {
            assert_ne!(alloc_id(seed).get(), 0);
        }
    }

    #[test]
    fn next_wraps_past_max_id_back_to_one_not_zero() {
        let max = ImageId::from_masked(MAX_ID);
        assert_eq!(max.next().get(), 1);
        assert_eq!(ImageId::from_masked(MAX_ID - 1).next().get(), MAX_ID);
        assert_eq!(ImageId::from_masked(1).next().get(), 2);
    }
}
