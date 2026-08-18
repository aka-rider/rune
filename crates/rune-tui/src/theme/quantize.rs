//! The 256-colour (xterm ANSI) quantizer: macOS
//! Terminal.app — the default terminal on the only OS this app supports —
//! has no truecolor support, so every `Theme` built for it needs its
//! `Color::Rgb` values mapped down to `Color::Indexed`. Applied exactly
//! once, at `Theme::catppuccin_mocha` construction (`theme/mod.rs`), never
//! per frame.

use ratatui::style::Color;

/// The 6 evenly-spaced intensity levels the xterm 256-colour cube uses per
/// channel — index `16 + 36r + 6g + b` for `r, g, b` each in `0..6`.
const CUBE_LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];

/// The 24-step greyscale ramp's values (indices `232..=255`): `8..=238`
/// step `10`.
const GREY_START: i32 = 8;
const GREY_STEP: i32 = 10;
const GREY_STEPS: u8 = 24;

/// The cube level index (`0..6`) nearest `v`.
fn nearest_cube_level(v: u8) -> u8 {
    let v = i32::from(v);
    let mut best_idx = 0u8;
    let mut best_dist = i32::MAX;
    for (i, &level) in CUBE_LEVELS.iter().enumerate() {
        let dist = (i32::from(level) - v).abs();
        if dist < best_dist {
            best_dist = dist;
            best_idx = i as u8;
        }
    }
    best_idx
}

fn squared_distance(a: (u8, u8, u8), b: (u8, u8, u8)) -> i32 {
    let dr = i32::from(a.0) - i32::from(b.0);
    let dg = i32::from(a.1) - i32::from(b.1);
    let db = i32::from(a.2) - i32::from(b.2);
    dr * dr + dg * dg + db * db
}

/// Maps a truecolor `Color::Rgb` down to the nearest `Color::Indexed`
/// candidate in xterm's 256-colour palette — the 6×6×6 cube (indices
/// `16..=231`) plus the 24-step grey ramp (indices `232..=255`) — by
/// minimizing Euclidean RGB distance over BOTH candidate sets. Any other
/// `Color` variant (already `Indexed`, a named ANSI colour, `Reset`)
/// passes through unchanged — there is nothing left to quantize.
pub fn to_ansi256(c: Color) -> Color {
    let Color::Rgb(r, g, b) = c else {
        return c;
    };

    let ri = nearest_cube_level(r);
    let gi = nearest_cube_level(g);
    let bi = nearest_cube_level(b);
    // `nearest_cube_level` only ever returns an index into `CUBE_LEVELS`
    // (`0..6`, one per `enumerate()`'d level), so `.get` never actually
    // misses — `unwrap_or(255)` (the cube's own brightest level) is a
    // defensive fallback that keeps this indexing-panic-free without
    // asserting an invariant the caller can't violate anyway.
    let cube_level = |idx: u8| CUBE_LEVELS.get(idx as usize).copied().unwrap_or(255);
    let cube_rgb = (cube_level(ri), cube_level(gi), cube_level(bi));
    let cube_index = 16 + 36 * ri + 6 * gi + bi;
    let cube_dist = squared_distance((r, g, b), cube_rgb);

    let mut best_grey_index = 232u8;
    let mut best_grey_dist = i32::MAX;
    for step in 0..GREY_STEPS {
        let level = (GREY_START + GREY_STEP * i32::from(step)).clamp(0, 255) as u8;
        let dist = squared_distance((r, g, b), (level, level, level));
        if dist < best_grey_dist {
            best_grey_dist = dist;
            best_grey_index = 232 + step;
        }
    }

    if cube_dist <= best_grey_dist {
        Color::Indexed(cube_index)
    } else {
        Color::Indexed(best_grey_index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mocha's `base` (`#1e1e2e` = rgb(30,30,46)) lands on the grey ramp,
    /// not the colour cube — its channels are close enough to each other
    /// (and far from every cube level) that a grey step wins on distance.
    #[test]
    fn quantizes_mocha_base_to_a_fixed_grey_index() {
        assert_eq!(
            to_ansi256(Color::Rgb(0x1e, 0x1e, 0x2e)),
            Color::Indexed(235)
        );
    }

    /// Mocha's `mauve` (`#cba6f7`) lands in the colour cube.
    #[test]
    fn quantizes_mocha_mauve_to_a_fixed_cube_index() {
        assert_eq!(
            to_ansi256(Color::Rgb(0xcb, 0xa6, 0xf7)),
            Color::Indexed(183)
        );
    }

    /// Mocha's `red` (`#f38ba8`) lands in the colour cube.
    #[test]
    fn quantizes_mocha_red_to_a_fixed_cube_index() {
        assert_eq!(
            to_ansi256(Color::Rgb(0xf3, 0x8b, 0xa8)),
            Color::Indexed(211)
        );
    }

    /// Anything that isn't `Color::Rgb` passes through untouched — an
    /// already-quantized value is idempotent under a second pass.
    #[test]
    fn non_rgb_colors_pass_through_unchanged() {
        assert_eq!(to_ansi256(Color::Indexed(42)), Color::Indexed(42));
        assert_eq!(to_ansi256(Color::Reset), Color::Reset);
    }
}
