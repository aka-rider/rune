//! The `Action` model: the fuzzer's input vocabulary. Modelled on the Go
//! fuzzer's event vocabulary, scoped to what Phase-1 Rust reaches
//! (no docstate/journal persistence, no file tree, no dictation — see the
//! plan's "Explicitly out of scope").
//!
//! There is no `DeliverMode` enum: G9 proves at most one save `Cmd` can ever
//! be outstanding (`trigger_save` guards on `save_in_flight`, `app.rs:328-
//! 330`), so a mode enum would just be three names for one behaviour
//! `[fixes R1]`.

use rune_syntax::ScopeId;
use rune_tui::keymap::KeyInput;
use rune_tui::runtime::DirCause;
use rune_vfs::DirEntry;

/// Which buffer generation a synthesized `Msg::Highlighted` reply should
/// claim, resolved against the LIVE buffer version at delivery time (never
/// a fixed constant — mirrors `Action::ConfirmTimeout`'s own rule, since
/// generation 0 is a real value): `Live` -> `buffer.version()`, `Stale` ->
/// `buffer.version().saturating_sub(1)`, `Future` -> `buffer.version() + 1`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HighlightVersion {
    Live,
    Stale,
    Future,
}

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
    /// Delivers `Msg::DirLoaded` with an arbitrary entry set, cause, and
    /// generation (plan WP4.S6; `generation` added by the review fix for
    /// `explorer::handle_dir_loaded`'s staleness guard) — the driver always
    /// targets a fixed root; only `entries`/`cause`/`generation` vary.
    /// Exercises the Explorer's dir-loaded handler against garbage input
    /// with no real `ReadDir` `Cmd` behind it, the same "deliver a
    /// synthesized reply directly" shape `ClipboardReply`/`ConfirmTimeout`
    /// already use. Unlike `ConfirmTimeout` (G15: must always target the
    /// LIVE armed generation), `generation` here is deliberately allowed to
    /// be an arbitrary, usually-stale value — `handle_dir_loaded` silently
    /// ignoring a reply that doesn't match `Explorer::request_generation`
    /// is exactly the property under fuzz, so pinning the generator to only
    /// ever emit the live value would stop exercising it.
    DirLoaded {
        entries: Vec<DirEntry>,
        cause: DirCause,
        generation: u32,
    },
    /// Synthesizes a `Msg::Highlighted` reply directly (plan WP7.S4) — the
    /// same "deliver a synthesized reply with no real `Cmd` behind it" shape
    /// `ClipboardReply`/`ConfirmTimeout`/`DirLoaded` already use. `version`
    /// is resolved against the LIVE buffer version at delivery time, never
    /// synthesized as a fixed constant (see `HighlightVersion`'s own docs).
    /// `spans` is a raw, DELIBERATELY unvalidated `(start, end, ScopeId)`
    /// triple list — out-of-bounds, inverted, and mid-`char` ranges are all
    /// legal generator output, since `dispatch::handle_highlighted`'s own
    /// clamping/discarding is exactly the property under fuzz.
    Highlight {
        version: HighlightVersion,
        spans: Vec<(usize, usize, u16)>,
    },
}

/// Rebuilds the concrete `(Range<usize>, ScopeId)` pairs `Msg::Highlighted`
/// carries from an `Action::Highlight`'s raw triples — shared by the driver
/// (constructing the real message) and the script codec (round-trip tests
/// never need this, but keeping the conversion in one place avoids two
/// copies of the same `u16 -> ScopeId` wrap).
pub fn highlight_spans_from_raw(
    spans: &[(usize, usize, u16)],
) -> Vec<(std::ops::Range<usize>, ScopeId)> {
    spans
        .iter()
        .map(|&(start, end, scope)| (start..end, ScopeId(scope)))
        .collect()
}
