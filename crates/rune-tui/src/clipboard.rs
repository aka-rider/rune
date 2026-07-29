//! OSC 52 clipboard writes and `pbpaste`-based clipboard reads (WP8, plan
//! Context "Clipboard"). Write: an OSC 52 escape sequence carrying the
//! base64-encoded payload — built here as plain bytes and pushed into
//! `Effects.raw` by `commands::clipboard`, never sent from a `Cmd` (plan
//! Gotchas: "Cmds must never touch the terminal"). Read: `/usr/bin/pbpaste`,
//! a deliberate deviation from Go's pure-OSC-52 read (plan Context
//! "Clipboard": OSC 52 *read* is unsupported in Terminal.app and permission-
//! gated in iTerm2/kitty — the macOS terminals this app targets; helix ships
//! exactly this hybrid).

use std::process::Command as ProcessCommand;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

use crate::runtime::{Cmd, CmdKind, Msg};

/// The largest raw (pre-base64) payload `osc52_copy` will encode (plan
/// WP13.S4, `rune-tui C 6`). Terminal multiplexers (tmux, screen) cap how
/// large an OSC 52 sequence they'll actually forward to the real terminal
/// — far below 1 MiB — and silently drop anything over their own limit, so
/// an unbounded copy/cut can write bytes into a sequence that never
/// reaches the system clipboard at all, with no signal to the user. Kept
/// comfortably under the smallest common multiplexer cap.
pub const OSC52_MAX_PAYLOAD_BYTES: usize = 100_000;

/// Builds the OSC 52 "set system clipboard" escape sequence for `payload`:
/// `ESC ] 5 2 ; c ; <base64> BEL` (plan Context "Clipboard": "exact Go
/// parity", `commands_clipboard.go`, mediated through Bubble Tea's
/// `tea.SetClipboard` there — the `c` selector targets the system
/// clipboard, not a primary/selection buffer). Pure and terminal-free: the
/// caller (`commands::clipboard::copy`/`cut`) pushes the returned bytes into
/// `Effects.raw`; this function performs no I/O itself.
pub fn osc52_copy(payload: &[u8]) -> Vec<u8> {
    let encoded = STANDARD.encode(payload);
    let mut out = Vec::with_capacity(encoded.len() + 8);
    out.extend_from_slice(b"\x1b]52;c;");
    out.extend_from_slice(encoded.as_bytes());
    out.push(0x07); // BEL
    out
}

/// The paste-read `Cmd`: runs `/usr/bin/pbpaste` on its own thread (every
/// `Cmd` runs off the main thread by runtime design — see `runtime.rs` —
/// and never touches the terminal) and reports its stdout back as
/// `Msg::ClipboardRead`. A failure to spawn pbpaste, a non-zero exit, or
/// stdout that isn't valid UTF-8 all produce `Msg::Error` instead of
/// silently dropping or mangling the paste, so the user sees why nothing
/// happened rather than nothing at all.
///
/// pbpaste's stdout is decoded STRICTLY, not lossily: a lossy decode would
/// silently substitute U+FFFD for invalid bytes and still hand the result
/// to `Msg::ClipboardRead`, which reaches `commands::clipboard::paste` and,
/// from there, the user's own buffer and eventually their file on
/// materialize — the ONE swallowed failure in an otherwise error-surfacing
/// path (`CODE-REVIEW.md` rune-tui B finding 8). Rejecting outright and
/// inserting nothing is the same trade this crate makes everywhere else
/// user-visible content could be silently altered.
pub fn pbpaste_cmd() -> Cmd {
    Cmd::new(CmdKind::ClipboardRead, || {
        let output = match ProcessCommand::new("/usr/bin/pbpaste").output() {
            Ok(output) => output,
            Err(e) => return Some(Msg::Error(format!("pbpaste failed to run: {e}"))),
        };
        if !output.status.success() {
            return Some(Msg::Error(format!(
                "pbpaste exited with status {}",
                output.status
            )));
        }
        Some(decode_pbpaste_stdout(output.stdout))
    })
}

/// The strict-decode chokepoint `pbpaste_cmd` reduces to, pulled out as its
/// own pure function so the invalid-UTF-8 path is unit-testable without
/// shelling out to a real `pbpaste`.
fn decode_pbpaste_stdout(stdout: Vec<u8>) -> Msg {
    match String::from_utf8(stdout) {
        Ok(text) => Msg::ClipboardRead(text),
        Err(_) => Msg::Error("pbpaste produced bytes that are not valid UTF-8".to_string()),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn osc52_copy_builds_the_exact_escape_sequence() {
        let bytes = osc52_copy(b"hi");
        let expected = format!("\x1b]52;c;{}\x07", STANDARD.encode(b"hi"));
        assert_eq!(bytes, expected.into_bytes());
    }

    #[test]
    fn osc52_copy_of_empty_payload_still_wraps_valid_escape_bytes() {
        let bytes = osc52_copy(b"");
        assert_eq!(bytes, b"\x1b]52;c;\x07".to_vec());
    }

    #[test]
    fn decode_pbpaste_stdout_passes_through_valid_utf8() {
        let msg = decode_pbpaste_stdout(b"hello".to_vec());
        let Msg::ClipboardRead(text) = msg else {
            unreachable!("expected a ClipboardRead message, got {msg:?}");
        };
        assert_eq!(text, "hello");
    }

    /// Regression for `CODE-REVIEW.md` rune-tui B finding 8: invalid UTF-8
    /// from the clipboard must surface as `Msg::Error` and insert nothing,
    /// never silently substitute U+FFFD and still hand the mangled text to
    /// `Msg::ClipboardRead`.
    #[test]
    fn decode_pbpaste_stdout_rejects_invalid_utf8_instead_of_substituting() {
        let invalid = vec![0xff, 0xfe, 0xfd];
        assert!(matches!(decode_pbpaste_stdout(invalid), Msg::Error(_)));
    }
}
