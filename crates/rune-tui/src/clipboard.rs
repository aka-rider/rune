use std::process::Command as ProcessCommand;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

use crate::runtime::{Cmd, Msg, PasteTarget};

// Terminal multiplexers (tmux, screen) cap how large an OSC 52 sequence they
// forward to the real terminal — far below 1 MiB — and silently drop
// anything over their own limit, so an unbounded copy/cut can write bytes
// into a sequence that never reaches the system clipboard at all, with no
// signal to the user. Kept comfortably under the smallest common
// multiplexer cap.
pub const OSC52_MAX_PAYLOAD_BYTES: usize = 100_000;

const OSC52_SET_SYSTEM_CLIPBOARD: &[u8] = b"\x1b]52;c;";
const BEL: u8 = 0x07;

pub fn osc52_copy(payload: &[u8]) -> Vec<u8> {
    let encoded = STANDARD.encode(payload);
    let mut out = Vec::with_capacity(encoded.len() + 8);
    out.extend_from_slice(OSC52_SET_SYSTEM_CLIPBOARD);
    out.extend_from_slice(encoded.as_bytes());
    out.push(BEL);
    out
}

// OSC 52 *read* is unsupported in Terminal.app and permission-gated in
// iTerm2/kitty — the macOS terminals this app targets — so paste shells out
// to `/usr/bin/pbpaste` instead; helix ships this same hybrid.
pub fn pbpaste_cmd(target: PasteTarget) -> Cmd {
    Cmd::clipboard_read(move || {
        let output = match ProcessCommand::new("/usr/bin/pbpaste").output() {
            Ok(output) => output,
            Err(e) => {
                return Some(Msg::Posted {
                    severity: crate::messages::Severity::Error,
                    text: format!("pbpaste failed to run: {e}"),
                });
            }
        };
        if !output.status.success() {
            return Some(Msg::Posted {
                severity: crate::messages::Severity::Error,
                text: format!("pbpaste exited with status {}", output.status),
            });
        }
        Some(decode_pbpaste_stdout(output.stdout, target))
    })
}

fn decode_pbpaste_stdout(stdout: Vec<u8>, target: PasteTarget) -> Msg {
    String::from_utf8(stdout).map_or_else(
        |_| Msg::Posted {
            severity: crate::messages::Severity::Error,
            text: "pbpaste produced bytes that are not valid UTF-8".to_string(),
        },
        |text| Msg::ClipboardRead { text, target },
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn doc_id() -> crate::document::DocumentId {
        use std::sync::Arc;
        let vfs: Arc<dyn rune_vfs::Vfs + Send + Sync> = Arc::new(rune_vfs::Mem::new());
        let mut app = crate::app::App::new(rune_core::buffer::Buffer::new(""), None, vfs, None);
        app.open_document(rune_core::buffer::Buffer::new(""))
    }

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
        let msg = decode_pbpaste_stdout(b"hello".to_vec(), PasteTarget::Title(doc_id()));
        let Msg::ClipboardRead { text, target } = msg else {
            unreachable!("expected a ClipboardRead message, got {msg:?}");
        };
        assert_eq!(text, "hello");
        assert_eq!(target, PasteTarget::Title(doc_id()));
    }

    #[test]
    fn decode_pbpaste_stdout_rejects_invalid_utf8_instead_of_substituting() {
        let invalid = vec![0xff, 0xfe, 0xfd];
        assert!(matches!(
            decode_pbpaste_stdout(invalid, PasteTarget::Title(doc_id())),
            Msg::Posted {
                severity: crate::messages::Severity::Error,
                ..
            }
        ));
    }
}
