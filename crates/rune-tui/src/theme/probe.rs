//! Terminal colour-depth detection (plan WP4.S5): macOS Terminal.app — the
//! default terminal on the only OS this app supports — has no truecolor
//! support, so `Theme` construction (`theme/mod.rs`) needs to know, before
//! it builds a single `Style`, whether the real terminal it's about to
//! draw into can render `Color::Rgb` at all.
//!
//! A device-attributes (`DA1`) round trip is the liveness check: write the
//! query, then poll with a bounded timeout for the typed `Csi::Device`
//! response `termina` parses for us. Terminals do not encode their colour
//! depth in a DA1 reply (there is no standard escape that does), so the
//! response only proves the other end is a real, responding terminal — not
//! a pipe or a non-interactive stream where writing an escape sequence and
//! blocking on a reply would hang forever. Once that's established (or the
//! query times out — no terminal attached at all), the actual truecolor
//! decision comes from `COLORTERM`, the de facto convention
//! `termstandard/colors` documents and every truecolor-capable terminal on
//! macOS (iTerm2, Ghostty, kitty) sets; Terminal.app never sets it.

use std::time::Duration;

use termina::escape::csi::{self, Csi};
use termina::{Event, Terminal};

/// How long to wait for the DA1 reply before assuming this stream will
/// never answer (no real terminal attached — non-interactive, redirected,
/// or piped) and falling straight through to the `COLORTERM` check.
const PROBE_TIMEOUT: Duration = Duration::from_millis(200);

/// `true` iff the terminal should be treated as truecolor-capable. Queries
/// `term` with a primary-device-attributes request as a responsiveness
/// gate (see module docs), then decides from `COLORTERM` regardless of
/// whether a reply arrived in time — a terminal that never replies
/// degrades to the `COLORTERM` fallback exactly like one that isn't
/// attached at all (never block indefinitely waiting on a query that
/// may never be answered).
pub fn supports_truecolor(term: &mut impl Terminal) -> bool {
    let _ = write!(
        term,
        "{}",
        Csi::Device(csi::Device::RequestPrimaryDeviceAttributes)
    );
    let _ = term.flush();
    let _ = term.poll(
        |ev| {
            matches!(
                ev,
                Event::Csi(Csi::Device(csi::Device::DeviceAttributes(_)))
            )
        },
        Some(PROBE_TIMEOUT),
    );
    colorterm_env_claims_truecolor()
}

/// `true` iff `value` is one of the two strings `termstandard/colors`
/// documents as meaning "this terminal supports 24-bit colour" — factored
/// out from [`colorterm_env_claims_truecolor`] so a test can exercise the
/// decision without mutating the real process environment (`COLORTERM` is
/// process-global; a test that set/unset it directly would race every
/// other test in this binary running concurrently). `pub(crate)` so
/// `graphics::caps::detect` can gate Kitty on the same decision instead of
/// duplicating it — the image id a placeholder cell smuggles through its
/// foreground colour is itself a 24-bit colour, so both features share one
/// truecolor question.
pub(crate) fn colorterm_claims_truecolor(value: Option<&str>) -> bool {
    matches!(value, Some("truecolor") | Some("24bit"))
}

fn colorterm_env_claims_truecolor() -> bool {
    colorterm_claims_truecolor(std::env::var("COLORTERM").ok().as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colorterm_truecolor_and_24bit_are_recognized() {
        assert!(colorterm_claims_truecolor(Some("truecolor")));
        assert!(colorterm_claims_truecolor(Some("24bit")));
    }

    #[test]
    fn anything_else_is_not_truecolor() {
        assert!(!colorterm_claims_truecolor(None));
        assert!(!colorterm_claims_truecolor(Some("")));
        assert!(!colorterm_claims_truecolor(Some("256color")));
    }
}
