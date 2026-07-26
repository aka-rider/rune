//! The `Action` model: the fuzzer's input vocabulary. Model on Go's
//! `internal/fuzz/event/event.go:6-54`, scoped to what Phase-1 Rust reaches
//! (no docstate/journal persistence, no file tree, no dictation — see the
//! plan's "Explicitly out of scope").
//!
//! There is no `DeliverMode` enum: G9 proves at most one save `Cmd` can ever
//! be outstanding (`trigger_save` guards on `save_in_flight`, `app.rs:328-
//! 330`), so a mode enum would just be three names for one behaviour
//! `[fixes R1]`.

use rune_tui::keymap::KeyInput;

/// One fuzzer-generated input. `driver::run` expands each `Action` into one
/// or more `Msg`s (`Type` expands per character) and delivers them through
/// the real `rune_tui::app::update`.
///
/// `Debug` is required — proptest demands `Strategy::Value: fmt::Debug`
/// (`proptest-1.11.0/src/strategy/traits.rs:37-46`), so a shrunk failing
/// case can be printed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// One keystroke, delivered as `Msg::Key`.
    Key(KeyInput),
    /// Typed text. Expanded per char by the driver: `'\n'` -> `KeyCode::
    /// Enter` (mods NONE), everything else -> `KeyCode::Char(c)`. Other
    /// control characters are UNREPRESENTABLE here — `is_insertable_key_
    /// char` would silently drop them (`app.rs:279-281`, plan Gotcha G3) —
    /// and the generator never emits them. Use `Paste` for byte-hostile
    /// payloads.
    Type(String),
    /// A bracketed paste, delivered as `Msg::Paste`. The ONLY path that
    /// inserts control bytes verbatim (G3).
    Paste(String),
    /// `Msg::Resize(w, h)`.
    Resize(u16, u16),
    /// Answer a pending `CmdKind::ClipboardRead` with this text
    /// (`Msg::ClipboardRead`) instead of forking pbpaste.
    ClipboardReply(String),
    /// Deliver `Msg::ConfirmTimeout` for the LIVE armed generation. A no-op
    /// when `app.pending_quit` is `None` — production can only ever
    /// deliver a timeout for a generation it armed, and generation 0 is a
    /// real value (`next_quit_gen` starts at 0), so it must never be
    /// synthesized as a fixed constant (G15).
    ConfirmTimeout,
    /// Run the one deferred `CmdKind::Save`, if any, and feed back its
    /// `Msg::SaveDone`. A no-op when no save is pending.
    Deliver,
    /// Arm `Mem::fail_next_save(ErrorKind::PermissionDenied)`.
    FailNextSave,
}
