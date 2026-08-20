pub trait EnvSource {
    fn var(&self, key: &str) -> Option<String>;
}

pub struct ProcessEnv;

impl EnvSource for ProcessEnv {
    fn var(&self, key: &str) -> Option<String> {
        std::env::var(key).ok()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GraphicsCaps {
    pub kitty: bool,
    pub cell: rune_image::CellSize,
}

impl Default for GraphicsCaps {
    fn default() -> Self {
        GraphicsCaps {
            kitty: false,
            cell: rune_image::DEFAULT_CELL_SIZE,
        }
    }
}

pub fn detect(env: &impl EnvSource, window: Option<(u16, u16, u16, u16)>) -> GraphicsCaps {
    let term_program = env.var("TERM_PROGRAM").unwrap_or_default().to_lowercase();
    let term = env.var("TERM").unwrap_or_default().to_lowercase();

    let mut kitty = if term_program.contains("kitty") {
        true
    } else {
        term_program.contains("ghostty") || term.contains("ghostty")
    };

    if !kitty {
        let non_empty_window_id = env.var("KITTY_WINDOW_ID").is_some_and(|id| !id.is_empty());
        if non_empty_window_id {
            kitty = true;
        }
    }

    let truecolor =
        crate::theme::probe::colorterm_claims_truecolor(env.var("COLORTERM").as_deref());
    let kitty = kitty && truecolor;

    GraphicsCaps {
        kitty,
        cell: measure_cell(window),
    }
}

fn measure_cell(window: Option<(u16, u16, u16, u16)>) -> rune_image::CellSize {
    // termina reports Some(0) rather than None when the kernel doesn't fill
    // in ws_xpixel/ws_ypixel, so 0 must be treated as "unavailable" too.
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
            self.0.get(key).map(std::string::ToString::to_string)
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
