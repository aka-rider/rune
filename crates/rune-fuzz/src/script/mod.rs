//! A hand-written, dependency-free line codec for `(path, content,
//! Vec<Action>)`. One action per line, first line always `content
//! <escaped>`:
//!
//! ```text
//! content <escaped>            # always the first line
//! path <escaped>                # OPTIONAL, only right after `content`; defaults to DOC_PATH
//! key <code> <mods>            # key char:a ----   |  key left s---  |  key char:\u{20} ---u
//! mouse <kind> <col> <row> <mods>   # mouse down:left 10 5 ---  |  mouse scroll-up 0 0 s-c
//! type <escaped>
//! paste <escaped>
//! resize <w> <h>
//! clip <escaped>
//! confirm-timeout
//! deliver
//! fail-next-save
//! dirloaded <nav|refresh> <generation>   # followed by 0+ continuation lines:
//! dirloaded-entry <f|d> <escaped name>
//! highlight <live|stale|future> <n>      # followed by exactly n continuation lines:
//! highlight-span <start> <end> <scope>
//! highlight-tree <live|stale|future> <fixture> <base>   # single line, no continuation
//! ```
//!
//! `dirloaded`/`dirloaded-entry` and `highlight`/`highlight-span` are the
//! MULTI-line actions (plan WP4.S6, WP7.S5): a `DirEntry`'s `name` is an
//! arbitrary `String` that may itself contain a literal space, so packing a
//! variable-length entry list onto one line with a space-joined delimiter
//! would be ambiguous — one continuation line per entry/span sidesteps
//! that instead of inventing a second escaping scheme.
//!
//! Deviation from the plan's grammar sketch: `Action` (`crate::action`) has
//! no `DeliverMode` — G9 proves at most one save can ever be outstanding —
//! so `deliver` is a bare token, never `deliver oldest|newest|all`.
//!
//! `key`'s `<mods>` is a fixed 4-char field (shift, alt, ctrl, sup), each
//! `-` or its initial letter. Decode locates it by taking the LAST 4
//! characters of the line plus the separating space before them, never by a
//! generic whitespace split — so an escaped `char:` payload may itself
//! contain a literal space with no ambiguity. `mouse`'s `<mods>` follows the
//! same convention minus `sup` (a mouse event carries no super modifier): a
//! fixed 3-char field, and since no `mouse` field is ever escaped, decode
//! splits it plainly.
//!
//! Text payloads are escaped with `char::escape_default()` — always ASCII:
//! printable ASCII passes through unescaped, everything else becomes one of
//! `\n \r \t \\ \' \"` or a `\u{HEX}` run. `unescape` accepts exactly that
//! set, plus `\0` (which `escape_default` itself emits as `\u{0}`, but a
//! hand-authored script may still use the mnemonic).
//!
//! No `unwrap`/`expect`/`panic!`/unchecked indexing across this module
//! (`encode`/`decode` split into their own files, G17) — every fallible
//! step returns a `ScriptError`, mirroring `rune_core::buffer::BufferError`'s
//! idiom.

mod decode;
mod decode_key;
mod encode;
mod keyword;

use std::fmt;

pub use decode::decode;
pub use encode::encode;

/// Why a script line could not be decoded. Never constructed by `encode` —
/// only ever returned by `decode` on malformed input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScriptError {
    /// No non-comment, non-blank `content` line was found.
    MissingContentLine,
    /// A line was structurally wrong in a way no more specific variant names.
    MalformedLine { line: usize, reason: String },
    /// An action line's first token matched no known keyword.
    UnknownKeyword { line: usize, keyword: String },
    /// A `\`-escape was unknown, truncated, or an invalid `\u{...}` value.
    InvalidEscape { line: usize, reason: String },
    /// A `key` line's code field matched no known `KeyCode` spelling.
    InvalidKeyCode { line: usize, code: String },
    /// A `key` line's mods field was not exactly 4 valid flag characters.
    InvalidMods { line: usize, mods: String },
    /// A numeric field did not parse as its expected integer type.
    InvalidNumber { line: usize, reason: String },
    /// A `type` line's payload decoded to a control char `driver::run`
    /// cannot deliver as a keystroke (CODE-REVIEW.md rune-fuzz finding 4):
    /// `Action::Type` expands one `char` per `Msg::Key`, and
    /// `is_insertable_key_char` silently drops any control byte other than
    /// `\n` (plan Gotcha G3) — a byte-hostile payload belongs in
    /// `Action::Paste` instead, which delivers verbatim. Rejected here,
    /// at decode time, rather than left for the driver's own
    /// `debug_assert!` to catch: that assert sits outside `catch_unwind`,
    /// so a repro carrying one would abort the whole replay harness
    /// instead of producing a named violation.
    UndeliverableTypeChar { line: usize, ch: char },
}

impl fmt::Display for ScriptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScriptError::MissingContentLine => write!(f, "script has no `content` line"),
            ScriptError::MalformedLine { line, reason }
            | ScriptError::InvalidEscape { line, reason }
            | ScriptError::InvalidNumber { line, reason } => write!(f, "line {line}: {reason}"),
            ScriptError::UnknownKeyword { line, keyword } => {
                write!(f, "line {line}: unknown action keyword {keyword:?}")
            }
            ScriptError::InvalidKeyCode { line, code } => {
                write!(f, "line {line}: invalid key code {code:?}")
            }
            ScriptError::InvalidMods { line, mods } => {
                write!(f, "line {line}: invalid mods field {mods:?}")
            }
            ScriptError::UndeliverableTypeChar { line, ch } => write!(
                f,
                "line {line}: `type` payload contains undeliverable control char {ch:?} \
                 (use `paste` instead)"
            ),
        }
    }
}

impl std::error::Error for ScriptError {}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests;
