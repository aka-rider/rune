//! Pure animation-timing math shared by the GIF decoder and the playback
//! scheduler.

use std::time::Duration;

/// The floor applied to GIF frame delays; many GIFs encode a 0/very-small
/// delay expecting the renderer to clamp.
const MIN_FRAME_DELAY: Duration = Duration::from_millis(50);

/// Converts a GIF delay (hundredths of a second) to a duration, flooring it
/// at [`MIN_FRAME_DELAY`]. A negative input is treated as zero.
pub fn clamp_delay(hundredths: i64) -> Duration {
    let d = Duration::from_millis(clamped_millis(hundredths));
    if d < MIN_FRAME_DELAY {
        MIN_FRAME_DELAY
    } else {
        d
    }
}

fn clamped_millis(hundredths: i64) -> u64 {
    u64::try_from(hundredths.max(0).saturating_mul(10)).unwrap_or(0)
}
