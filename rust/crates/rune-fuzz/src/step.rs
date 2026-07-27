//! Tier-2 step context: the owned data a `Snapshot` structurally cannot
//! hold `[fixes B3]`. `rune_tui::runtime::Msg` derives nothing and owns a
//! `String`/`Result`, so it can't be stored or compared by a checker — the
//! driver instead tags each message it delivers with an owned `MsgTag` at
//! construction time (never by a totalizing `From<&Msg>`, since the driver
//! never delivers every `Msg` variant — e.g. `Msg::Error`/`Msg::Quit` never
//! flow through this headless driver, see `driver.rs`'s module docs).
//!
//! Mirrors how Go's driver passes `(rs, m, msg, prev, snap)` to its L2
//! checks (`internal/fuzz/driver/driver_verbatim.go:25`).

use rune_tui::keymap::{Command, KeyInput};

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
    ClipboardRead(String),
    SaveDone {
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
    Quit,
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
    /// `MsgTag::SaveDone` step. Pins `SAVE-VERBATIM` (a later work
    /// package).
    pub delivered_save_bytes: Option<Vec<u8>>,
    pub saves_delivered_ok: usize,
}
