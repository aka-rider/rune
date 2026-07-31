//! Deterministic Kitty image IDs derived from a document's absolute path.

const FNV_OFFSET_BASIS: u32 = 2_166_136_261;
const FNV_PRIME: u32 = 16_777_619;
const ID_MASK_24: u32 = 0x00FF_FFFF;

/// Derives a deterministic, non-zero, 24-bit image ID from an absolute path
/// using FNV-1a. The 24-bit bound lets the ID be encoded in a single
/// truecolor foreground value for Unicode placeholder cells.
pub fn alloc_id(abs_path: &str) -> u32 {
    let mut hash = FNV_OFFSET_BASIS;
    for byte in abs_path.as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    let id = hash & ID_MASK_24;
    if id == 0 { 1 } else { id }
}

/// The seed string hashed to derive one animation frame's image ID. The
/// reference implementation derives frame IDs by hashing exactly this
/// string.
pub fn frame_id_seed(abs_path: &str, frame: usize) -> String {
    format!("{abs_path}#frame{frame}")
}
