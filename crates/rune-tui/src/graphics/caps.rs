//! Terminal graphics capability detection (plan WP3.S1/S2/S3) — an
//! environment-only sniff, mirroring `golang/pkg/terminal/terminal.go`
//! exactly, plus the measured cell pixel geometry an image needs to size
//! itself in columns and rows.

/// A source of environment variables, so [`detect`] is unit-testable
/// without mutating the real process environment (`std::env::var` is
/// process-global — a test that set/unset a variable directly would race
/// every other test in this binary running concurrently, exactly the
/// reason `theme::probe` factored its own COLORTERM decision out from the
/// real env read).
pub trait EnvSource {
    fn var(&self, key: &str) -> Option<String>;
}

/// Reads from the real process environment — the only production
/// implementation of [`EnvSource`].
pub struct ProcessEnv;

impl EnvSource for ProcessEnv {
    fn var(&self, key: &str) -> Option<String> {
        std::env::var(key).ok()
    }
}

/// This process's view of the terminal's graphics support: whether the
/// Kitty graphics protocol is usable, and the measured (or fallback) pixel
/// size of one terminal cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GraphicsCaps {
    pub kitty: bool,
    pub cell: rune_image::CellSize,
}

impl Default for GraphicsCaps {
    /// No Kitty support, `rune_image::DEFAULT_CELL_SIZE` geometry — so
    /// every existing test constructor (`App::new`/`App::new_untitled`)
    /// keeps compiling unchanged, and the fuzzer stays deterministic
    /// exactly like `App::space_probe`'s `NullProbe` default.
    fn default() -> Self {
        GraphicsCaps {
            kitty: false,
            cell: rune_image::DEFAULT_CELL_SIZE,
        }
    }
}

/// Detects graphics capability from the environment and the terminal's
/// reported window geometry, mirroring `golang/pkg/terminal/terminal.go`'s
/// `DetectWithProber` exactly (not the prose summary an earlier plan
/// revision gave — that description was wrong).
///
/// `window` is `(cols, rows, pixel_width, pixel_height)`, as reported by
/// the backend's own dimensions query.
pub fn detect(env: &impl EnvSource, window: Option<(u16, u16, u16, u16)>) -> GraphicsCaps {
    let term_program = env.var("TERM_PROGRAM").unwrap_or_default().to_lowercase();
    let term = env.var("TERM").unwrap_or_default().to_lowercase();

    // First-match switch, exactly as Go's `switch` does it: `kitty` in
    // TERM_PROGRAM wins outright; otherwise `ghostty` in either
    // TERM_PROGRAM or TERM (Ghostty implements the Kitty graphics protocol,
    // including the Unicode-placeholder virtual-placement extension).
    let mut kitty = if term_program.contains("kitty") {
        true
    } else {
        term_program.contains("ghostty") || term.contains("ghostty")
    };

    // Then, unconditionally: KITTY_WINDOW_ID promotes to Kitty when no
    // protocol was detected above. Go's promotion arm has NO TERM_PROGRAM
    // allow-list — that gate exists only on the (out-of-scope here)
    // WEZTERM_PANE/ITERM_SESSION_ID arms — so `TERM_PROGRAM=vscode` with
    // `KITTY_WINDOW_ID` set still promotes.
    if !kitty {
        let non_empty_window_id = env.var("KITTY_WINDOW_ID").is_some_and(|id| !id.is_empty());
        if non_empty_window_id {
            kitty = true;
        }
    }

    // Truecolor is a hard gate on the whole result: the smuggled image id
    // IS a 24-bit colour. Reuse `theme::probe`'s COLORTERM decision rather
    // than duplicating it.
    let truecolor =
        crate::theme::probe::colorterm_claims_truecolor(env.var("COLORTERM").as_deref());
    let kitty = kitty && truecolor;

    GraphicsCaps {
        kitty,
        cell: measure_cell(window),
    }
}

/// `(pixel_width / cols, pixel_height / rows)`, falling back to
/// `rune_image::DEFAULT_CELL_SIZE` when any of the four inputs is `0` or
/// either quotient is `0`. termina always reports `Some(0)` rather than
/// `None` when the kernel doesn't fill in `ws_xpixel`/`ws_ypixel`, so `0`
/// must be treated as "unavailable" alongside a missing `window` entirely.
fn measure_cell(window: Option<(u16, u16, u16, u16)>) -> rune_image::CellSize {
    window
        .and_then(|(cols, rows, pixel_width, pixel_height)| {
            if cols == 0 || rows == 0 || pixel_width == 0 || pixel_height == 0 {
                return None;
            }
            let w = pixel_width as usize / cols as usize;
            let h = pixel_height as usize / rows as usize;
            if w == 0 || h == 0 {
                None
            } else {
                Some(rune_image::CellSize { w, h })
            }
        })
        .unwrap_or(rune_image::DEFAULT_CELL_SIZE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct FakeEnv(HashMap<&'static str, &'static str>);

    impl FakeEnv {
        fn new(pairs: &[(&'static str, &'static str)]) -> Self {
            FakeEnv(pairs.iter().copied().collect())
        }
    }

    impl EnvSource for FakeEnv {
        fn var(&self, key: &str) -> Option<String> {
            self.0.get(key).map(|v| v.to_string())
        }
    }

    const TRUECOLOR: (&str, &str) = ("COLORTERM", "truecolor");

    #[test]
    fn term_program_ghostty_is_kitty() {
        let env = FakeEnv::new(&[("TERM_PROGRAM", "ghostty"), TRUECOLOR]);
        assert!(detect(&env, None).kitty);
    }

    #[test]
    fn term_xterm_ghostty_with_empty_term_program_is_kitty() {
        let env = FakeEnv::new(&[("TERM_PROGRAM", ""), ("TERM", "xterm-ghostty"), TRUECOLOR]);
        assert!(detect(&env, None).kitty);
    }

    #[test]
    fn term_program_kitty_is_kitty() {
        let env = FakeEnv::new(&[("TERM_PROGRAM", "kitty"), TRUECOLOR]);
        assert!(detect(&env, None).kitty);
    }

    #[test]
    fn kitty_window_id_with_empty_term_program_is_kitty() {
        let env = FakeEnv::new(&[("TERM_PROGRAM", ""), ("KITTY_WINDOW_ID", "1"), TRUECOLOR]);
        assert!(detect(&env, None).kitty);
    }

    /// Matches Go exactly: the `KITTY_WINDOW_ID` promotion arm carries no
    /// `TERM_PROGRAM` allow-list, unlike the (out-of-scope)
    /// WEZTERM_PANE/ITERM_SESSION_ID arms.
    #[test]
    fn kitty_window_id_with_vscode_term_program_is_still_kitty() {
        let env = FakeEnv::new(&[
            ("TERM_PROGRAM", "vscode"),
            ("KITTY_WINDOW_ID", "1"),
            TRUECOLOR,
        ]);
        assert!(detect(&env, None).kitty);
    }

    #[test]
    fn ghostty_without_truecolor_is_not_kitty() {
        let env = FakeEnv::new(&[("TERM_PROGRAM", "ghostty")]);
        assert!(!detect(&env, None).kitty);
    }

    #[test]
    fn empty_environment_is_not_kitty() {
        let env = FakeEnv::new(&[]);
        assert!(!detect(&env, None).kitty);
    }

    #[test]
    fn measured_cell_size_divides_pixels_by_cells() {
        let env = FakeEnv::new(&[]);
        let caps = detect(&env, Some((80, 24, 1280, 768)));
        assert_eq!(caps.cell, rune_image::CellSize { w: 16, h: 32 });
    }

    #[test]
    fn zero_pixel_dims_fall_back_to_default_cell_size() {
        let env = FakeEnv::new(&[]);
        let caps = detect(&env, Some((80, 24, 0, 0)));
        assert_eq!(caps.cell, rune_image::DEFAULT_CELL_SIZE);
    }

    #[test]
    fn no_window_falls_back_to_default_cell_size() {
        let env = FakeEnv::new(&[]);
        assert_eq!(detect(&env, None).cell, rune_image::DEFAULT_CELL_SIZE);
    }
}
