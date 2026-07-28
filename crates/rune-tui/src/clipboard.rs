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

/// Builds the OSC 52 "set system clipboard" escape sequence for `payload`:
/// `ESC ] 5 2 ; c ; <base64> BEL` (plan Context "Clipboard": "exact Go
/// parity", `commands_clipboard.go:13-25`, mediated through Bubble Tea's
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
/// `Msg::ClipboardRead`. A failure to spawn pbpaste, or a non-zero exit,
/// produces `Msg::Error` instead of silently dropping the paste, so the
/// user sees why nothing happened rather than nothing at all.
///
/// pbpaste's stdout is decoded LOSSILY rather than dropped whole on invalid
/// UTF-8: the source is the external, user-controlled OS clipboard, not the
/// user's own file on disk — CONSTITUTION §1.4.5's byte-verbatim guarantee
/// governs what this app WRITES to the user's file, not arbitrary external
/// input it reads. Replacing a stray invalid byte with U+FFFD and still
/// pasting the rest of the text is strictly more useful than discarding an
/// entire paste over one bad byte from some other application.
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
        let text = String::from_utf8_lossy(&output.stdout).into_owned();
        Some(Msg::ClipboardRead(text))
    })
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
}
