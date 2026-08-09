//! Tier-2 step context: the owned data a `Snapshot` structurally cannot
//! hold `[fixes B3]`. `rune_tui::runtime::Msg` derives nothing and owns a
//! `String`/`Result`, so it can't be stored or compared by a checker — the
//! driver instead tags each message it delivers with an owned `MsgTag` at
//! construction time (never by a totalizing `From<&Msg>`, since the driver
//! never delivers every `Msg` variant — e.g. `Msg::Error`/`Msg::Quit` never
//! flow through this headless driver, see `driver.rs`'s module docs).

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
    /// `Msg::RenameDone` (plan WP5, fixing a driver gap: a rename `Cmd`
    /// used to be spawned and then simply dropped, since only `CmdKind::
    /// Save` was ever tracked — leaving `RenameState::Committing` stuck
    /// forever and permanently vetoing every later blur, including the
    /// end-of-session drive's own `^E`). No checker keys off this yet; the
    /// point of driving it is that `update` never panics and that a
    /// pending rename actually resolves within a session, the same
    /// precondition `discharge_pending_save` already established for
    /// saves.
    RenameDone,
    /// `Msg::Highlighted` (plan WP7.S4) — `delivered_version` is the version
    /// the driver actually stamped on the message (resolved from
    /// `HighlightVersion` against the live buffer at delivery time, not the
    /// raw enum tag itself); `span_count` is how many raw spans the
    /// generator attached, kept for report readability. `Action::
    /// HighlightTree` replies reuse this same tag with `span_count: 0` —
    /// its spans only exist at render-time query, so there is nothing to
    /// count at delivery. `HL-STALE-DROP`/`HL-NO-REFLOW`
    /// (`invariant/highlight.rs`) key off this variant.
    Highlighted {
        delivered_version: u64,
        span_count: usize,
    },
    /// `Msg::Db` — the oldest pending recovery-store op's reply, drained by
    /// `Action::DeliverDb` or the end-of-session sweep. `op_id` is the op
    /// this reply answers, kept for report readability. `doc` is the
    /// document `App::db_ops` named `op_id` for, read by the driver BEFORE
    /// delivery (`drain_one_db_op` — `handle_db_event` pops the entry as
    /// part of routing the ack, so it's gone by the time a checker could
    /// otherwise ask) — `None` only for a `DbEvent::Fatal` (kills the whole
    /// writer FIFO, not any one op) or a stale id with no live entry. Most
    /// checkers still don't key off this variant at all (the merge
    /// invariants key off `Snapshot::merge_active`/`merge_unresolved`
    /// instead); `SAVE-INFLIGHT-SM` is the one exception, using `doc` to
    /// recognize a store-backed save completing without trusting anything
    /// private to `materialize_ack`.
    Db {
        op_id: u64,
        doc: Option<DocumentId>,
    },
    /// `Msg::MaterializeVfsDone` — the caller-side `vfs` `Cmd` WP7's
    /// materialize dance spawns (`materialize_ack::materialize_vfs_cmd`)
    /// finishing, discharged by `Action::Deliver` alongside the no-store
    /// `SaveDone`/rename `Cmd`s (`discharge_pending_save`). `id` is the
    /// document the `Cmd` was built for — carried on the `Msg` itself, so
    /// unlike `Db` above there's no separate bookkeeping to consult.
    /// `SAVE-INFLIGHT-SM` uses it to recognize the two outcomes that settle
    /// `save_in_flight` synchronously, with no further `Db` round trip
    /// (`Missing`, and a local `vfs`/path-disagreement failure) — every
    /// other outcome (`Conflict`/`Committed`/`Raced`) instead enqueues a
    /// `MaterializeRecord` op, so `save_in_flight` doesn't change until the
    /// `Db` ack for THAT lands.
    MaterializeVfsDone {
        id: DocumentId,
    },
    /// `Msg::TrashDone` (plan WP3.S3, mirroring `RenameDone`'s driver gap
    /// fix): a `CmdKind::Trash` used to be silently dropped by the
    /// classification loop, so `Mem::trash` and this reply were unreachable
    /// from the fuzzer. No checker keys off this yet; the point of driving
    /// it is that `update` never panics on a trash reply and that a pending
    /// trash actually resolves within a session.
    TrashDone,
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
    /// Whether the ACTIVE document is still the one `disk`/`saves_delivered_
    /// ok` describe (`State::seed_doc`/`State::path`) — plan WP0 (`rr`
    /// history): closing the last open document now mints and activates a
    /// fresh untitled draft instead of refusing, so "active" and "the one
    /// seeded document this whole session is scoped to" can diverge even
    /// outside the Help-toggle case `save_clean_matches_disk`'s old
    /// `!read_only` proxy stood in for. Computed once per step so that
    /// checker never has to re-derive doc identity from a `Snapshot` that
    /// structurally can't carry it (module docs).
    pub active_is_seed_doc: bool,
}
