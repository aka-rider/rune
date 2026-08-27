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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_delay_floors_zero_hundredths_at_the_minimum_frame_delay() {
        assert_eq!(clamp_delay(0), MIN_FRAME_DELAY);
    }

    #[test]
    fn clamp_delay_floors_one_hundredth_at_the_minimum_frame_delay() {
        assert_eq!(clamp_delay(1), MIN_FRAME_DELAY);
    }

    #[test]
    fn clamp_delay_floors_negative_hundredths_at_the_minimum_frame_delay() {
        assert_eq!(clamp_delay(-5), MIN_FRAME_DELAY);
    }

    #[test]
    fn clamp_delay_at_five_hundredths_lands_exactly_on_the_floor_boundary() {
        assert_eq!(clamp_delay(5), Duration::from_millis(50));
    }

    #[test]
    fn clamp_delay_scales_hundredths_to_milliseconds_above_the_floor() {
        assert_eq!(clamp_delay(200), Duration::from_millis(2000));
    }
}
