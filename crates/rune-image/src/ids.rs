use std::num::NonZeroU32;

const FNV_OFFSET_BASIS: u32 = 2_166_136_261;
const FNV_PRIME: u32 = 16_777_619;
const ID_MASK_24: u32 = 0x00FF_FFFF;

pub const MAX_ID: u32 = 0x00FF_FFFF;

// Kitty: image id 0 means "no id", so ids here are always non-zero and
// wrap from MAX_ID back to 1. They're masked to 24 bits so one fits in a
// single truecolor foreground value for Unicode placeholder cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ImageId(NonZeroU32);

impl ImageId {
    fn from_masked(id: u32) -> ImageId {
        ImageId(NonZeroU32::new(id).unwrap_or(NonZeroU32::MIN))
    }

    pub fn next(self) -> ImageId {
        if self.0.get() >= MAX_ID {
            ImageId::from_masked(1)
        } else {
            ImageId::from_masked(self.0.get() + 1)
        }
    }

    pub fn get(self) -> u32 {
        self.0.get()
    }

    pub fn for_test(id: u32) -> ImageId {
        ImageId::from_masked(id)
    }
}

pub fn alloc_id(abs_path: &[u8]) -> ImageId {
    let mut hash = FNV_OFFSET_BASIS;
    for byte in abs_path {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    ImageId::from_masked(hash & ID_MASK_24)
}

pub fn frame_id_seed(abs_path: &str, frame: usize) -> String {
    format!("{abs_path}#frame{frame}")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn alloc_id_never_produces_the_reserved_zero_id() {
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

    #[test]
    fn frame_id_seed_is_deterministic_for_the_same_path_and_frame() {
        assert_eq!(frame_id_seed("/a/b.gif", 3), frame_id_seed("/a/b.gif", 3));
    }

    #[test]
    fn frame_id_seed_differs_across_frames_of_the_same_path() {
        assert_ne!(frame_id_seed("/a/b.gif", 3), frame_id_seed("/a/b.gif", 4));
    }

    #[test]
    fn frame_id_seed_differs_across_paths_at_the_same_frame() {
        assert_ne!(frame_id_seed("/a/b.gif", 3), frame_id_seed("/a/c.gif", 3));
    }

    #[test]
    fn frame_id_seed_is_built_from_the_path_and_frame_it_is_given() {
        let seed = frame_id_seed("/a/b.gif", 3);
        assert!(seed.contains("/a/b.gif"));
        assert!(seed.contains('3'));
    }
}
