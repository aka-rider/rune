use std::time::Duration;

const MIN_FRAME_DELAY: Duration = Duration::from_millis(50);

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
