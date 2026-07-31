//! Tier-2 step context: the owned data a `Snapshot` structurally cannot
//! hold `[fixes B3]`. `rune_tui::runtime::Msg` derives nothing and owns a
//! `String`/`Result`, so it can't be stored or compared by a checker — the
//! driver instead tags each message it delivers with an owned `MsgTag` at
//! construction time (never by a totalizing `From<&Msg>`, since the driver
//! never delivers every `Msg` variant — e.g. `Msg::Error`/`Msg::Quit` never
//! flow through this headless driver, see `driver.rs`'s module docs).
//!
//! Mirrors how Go's driver passes `(rs, m, msg, prev, snap)` to its L2
//! checks.

use rune_tui::document::DocumentId;
use rune_tui::keymap::{Command, KeyInput};
use rune_tui::runtime::PasteTarget;

/// Which message the driver just settled, tagged with everything a checker
/// needs but `Msg` itself can't carry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MsgTag {
    /// `command` is `keymap::resolve(input)` — `None` for a hardcoded fast
    /// path (Enter, Escape) or an unbound chord that fell through to
    /// plain-char insertion.
    Key {
        input: KeyInput,
        command: Option<Command>,
    },
    Paste(String),
    Resize(u16, u16),
    /// `target` is the `PasteTarget` captured when the driver spawned the
    /// `Cmd` this reply answers — never recovered from the reply itself,
    /// since the classification loop that inspects a spawned `Cmd` keeps
    /// only its `CmdKind`.
    ClipboardRead {
        text: String,
        target: PasteTarget,
    },
    SaveDone {
        /// The document `Msg::SaveDone` was actually FOR (`save::save_cmd`
        /// closes over it, `dispatch` forwards it untouched) — never
        /// assume this is whichever document happens to be `active` right
        /// now: a Guard modal's own `s`/`S` hotkey (`banner::handle_
        /// dirty_close_key`) can save a document OTHER than the active one
        /// (its own prompt's `doc`), and by the time this ack lands the
        /// active document may have changed again besides.
        id: DocumentId,
        version: u64,
        ok: bool,
    },
    ConfirmTimeout {
        generation: u32,
    },
    /// `Msg::DirLoaded` (plan WP4.S6) — no checker keys off this yet; the
    /// point of driving it is simply that `update` never panics and never
    /// touches the active document (proved structurally: `explorer::
    /// handle_dir_loaded` only ever writes `App::explorer`).
    DirLoaded,
    /// `Msg::Highlighted` (plan WP7.S4) — `delivered_version` is the version
    /// the driver actually stamped on the message (resolved from
    /// `HighlightVersion` against the live buffer at delivery time, not the
    /// raw enum tag itself); `span_count` is how many raw spans the
    /// generator attached, kept for report readability. `HL-STALE-DROP`/
    /// `HL-NO-REFLOW` (`invariant/highlight.rs`) key off this variant.
    Highlighted {
        delivered_version: u64,
        span_count: usize,
    },
}

/// Everything an invariant checker needs beyond `Snapshot`: what happened,
/// what left the process, and what is on disk. Hand-constructible like
/// `Snapshot`, so every checker — including ones added by a later work
/// package — gets both a positive and a negative test (plan Risk R-c).
#[derive(Clone, Debug)]
pub struct StepCtx {
    pub step: usize,
    pub msg: MsgTag,
    /// `effects.raw` produced by THIS message (OSC 52 bytes).
    pub raw: Vec<Vec<u8>>,
    /// `mem.read(&path)`; `None` means never saved (`ErrorKind::NotFound`,
    /// G16) — a real I/O error from this in-memory double is otherwise
    /// unreachable here, since only `save_atomic` (not `read`) ever
    /// consults the one-shot fault injector.
    pub disk: Option<Vec<u8>>,
    /// Bytes the pending `save_cmd` was handed at construction, if one is
    /// deferred right now.
    pub pending_save_bytes: Option<Vec<u8>>,
    /// Bytes the save that JUST completed was handed — set only on a
    /// `MsgTag::SaveDone` step, looked up by THAT ack's own `id`, never by
    /// whichever document happens to be active when the ack lands (see
    /// `MsgTag::SaveDone`'s own docs). Pins `SAVE-VERBATIM`.
    pub delivered_save_bytes: Option<Vec<u8>>,
    pub saves_delivered_ok: usize,
}
