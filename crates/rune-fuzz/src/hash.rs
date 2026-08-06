//! The one FNV-1a-32 implementation this crate uses to name artifact
//! directories deterministically from their own content — shared by
//! `report::write` (a caught invariant violation) and `wal::sweep` (a
//! promoted write-ahead script), so both name their bundle the same way
//! from the same encoded script.

/// FNV-1a, 32-bit. Hand-written (no new dependency).
pub(crate) fn fnv1a32(bytes: &[u8]) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for &b in bytes {
        hash ^= u32::from(b);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}
