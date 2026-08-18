//! Catppuccin Mocha's canonical truecolor palette, sourced
//! from the MIT `catppuccin` crate's generated `PALETTE.mocha.colors`
//! rather than hand-copied hex literals, so this crate's colours stay
//! byte-identical to upstream's own spec.

use ratatui::style::Color;

fn rgb(c: catppuccin::Rgb) -> Color {
    Color::Rgb(c.r, c.g, c.b)
}

/// The Catppuccin Mocha palette as `ratatui::style::Color::Rgb` — one
/// field per Catppuccin colour name (not per use site), so a style can
/// reuse e.g. `blue` or `mauve` without re-deriving it from the upstream
/// crate's own `Color`/`Rgb` types.
#[derive(Clone, Copy, Debug)]
pub struct Mocha {
    pub rosewater: Color,
    pub flamingo: Color,
    pub pink: Color,
    pub mauve: Color,
    pub red: Color,
    pub maroon: Color,
    pub peach: Color,
    pub yellow: Color,
    pub green: Color,
    pub teal: Color,
    pub sky: Color,
    pub sapphire: Color,
    pub blue: Color,
    pub lavender: Color,
    pub text: Color,
    pub subtext1: Color,
    pub subtext0: Color,
    pub overlay2: Color,
    pub overlay1: Color,
    pub overlay0: Color,
    pub surface2: Color,
    pub surface1: Color,
    pub surface0: Color,
    pub base: Color,
    pub mantle: Color,
    pub crust: Color,
}

impl Mocha {
    /// Reads every field straight from `catppuccin::PALETTE.mocha.colors`
    /// (`const`, generated at the `catppuccin` crate's OWN build time from
    /// upstream's palette spec) — never a hand-copied hex literal.
    pub fn palette() -> Mocha {
        let colors = catppuccin::PALETTE.mocha.colors;
        Mocha {
            rosewater: rgb(colors.rosewater.rgb),
            flamingo: rgb(colors.flamingo.rgb),
            pink: rgb(colors.pink.rgb),
            mauve: rgb(colors.mauve.rgb),
            red: rgb(colors.red.rgb),
            maroon: rgb(colors.maroon.rgb),
            peach: rgb(colors.peach.rgb),
            yellow: rgb(colors.yellow.rgb),
            green: rgb(colors.green.rgb),
            teal: rgb(colors.teal.rgb),
            sky: rgb(colors.sky.rgb),
            sapphire: rgb(colors.sapphire.rgb),
            blue: rgb(colors.blue.rgb),
            lavender: rgb(colors.lavender.rgb),
            text: rgb(colors.text.rgb),
            subtext1: rgb(colors.subtext1.rgb),
            subtext0: rgb(colors.subtext0.rgb),
            overlay2: rgb(colors.overlay2.rgb),
            overlay1: rgb(colors.overlay1.rgb),
            overlay0: rgb(colors.overlay0.rgb),
            surface2: rgb(colors.surface2.rgb),
            surface1: rgb(colors.surface1.rgb),
            surface0: rgb(colors.surface0.rgb),
            base: rgb(colors.base.rgb),
            mantle: rgb(colors.mantle.rgb),
            crust: rgb(colors.crust.rgb),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pins Mocha's `base` colour to the well-known hex `#1e1e2e` (the
    /// value every Catppuccin Mocha reference — and this theme's own
    /// `quantize` test — cites), guarding against the upstream crate ever
    /// shipping a differently-ordered or mis-mapped palette silently.
    #[test]
    fn mocha_base_matches_the_well_known_hex() {
        let m = Mocha::palette();
        assert_eq!(m.base, Color::Rgb(0x1e, 0x1e, 0x2e));
    }
}
