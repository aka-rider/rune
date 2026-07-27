//! `App`: the Elm-style model. `update` is the ONLY writer of synchronous
//! state (CONSTITUTION §5.4: "mutate synchronous state directly in
//! `update`; a Cmd is exclusively for I/O that leaves the thread").

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use rune_core::buffer::Buffer;
use rune_db::{DbEvent, MatResult, OpOutcome};
use rune_md::element::doc::ViewSnapshots;
use rune_vfs::Vfs;

use crate::commands::{clipboard, edit, nav};
use crate::db::{self, AppDb};
use crate::editor::Editor;
use crate::keymap::{self, Command, KeyCode, KeyInput, Mods, QuitKey};
use crate::runtime::{Cmd, CmdKind, Effects, Msg};

/// The quit-confirm arm-to-quit window (plan Context, "Quit-confirm": "first
/// press arms + spawns 2s timer Cmd carrying gen").
const CONFIRM_TIMEOUT: Duration = Duration::from_secs(2);

/// The degraded-save confirm-gate's arm-to-confirm window — mirrors
/// `CONFIRM_TIMEOUT` (plan WP5.S2/S6: "a pending-confirm state like the
/// existing quit-confirm pattern").
const SAVE_CONFIRM_TIMEOUT: Duration = Duration::from_secs(2);

/// The snapshot-autosave debounce window (plan WP5.S6, port of
/// `workspace_timers.go:11`'s 2s debounce).
const SNAPSHOT_DEBOUNCE: Duration = Duration::from_secs(2);

/// Which subsystem last wrote `App::status_message` — the provenance tag
/// `Msg::SaveDone`'s success arm needs so it clears ONLY a message its own
/// save path set, never an unrelated one (review finding F2: an earlier
/// version cleared `status_message` unconditionally on every successful
/// save, stomping e.g. an unresolved "pbpaste failed" error the user hadn't
/// dismissed yet). The ORIGINAL status-message ownership rule (F2 in
/// `commands::edit`: a successful edit/undo/redo must never clear an
/// unrelated message) still holds unchanged — those call sites only ever
/// WRITE `status_message`, they never clear it, so they need no provenance
/// tag for that rule; they still tag their writes below so a stale write
/// from one of them can never be mistaken for `SaveError` and get swept up
/// by a later successful save.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StatusSource {
    /// A failed (or un-attempted, e.g. "no file to save") save attempt —
    /// the ONLY source a successful `Msg::SaveDone` is allowed to clear.
    SaveError,
    /// Everything else: edit/undo/redo failures, `Msg::Error` (a pbpaste
    /// failure, a caught background-`Cmd` panic, the input stream ending),
    /// ...
    #[default]
    Other,
}

/// The whole editor model: the single editing pane (Phase 1 is one file, one
/// pane), file identity, the injected `Vfs` save target, and app-wide UI
/// state (status message, quit-confirm arming) that doesn't belong to any
/// one editing pane.
pub struct App {
    pub editor: Editor,
    pub file_path: Option<PathBuf>,
    pub vfs: Arc<dyn Vfs + Send + Sync>,
    /// The buffer version the LAST successful save/materialize ack
    /// persisted — advanced ONLY from a store ack (`handle_materialize_ack`)
    /// or, for the no-store fallback path, `Msg::SaveDone` (see
    /// `trigger_save`'s docs). Never read directly by `is_dirty` — see
    /// `is_dirty_cached`.
    pub saved_version: u64,
    /// The version `materialize`/the fallback save `Cmd` targets while a
    /// save is in flight — carried so its eventual ack only ever advances
    /// `saved_version` to the version IT captured, never the buffer's
    /// current (possibly further-edited) version (mirrors the pre-WP5
    /// `Msg::SaveDone { version, .. }` field, now also needed for the
    /// `materialize` path, whose ack carries no version of its own).
    pub save_pending_version: Option<u64>,
    pub save_in_flight: bool,
    /// The render-only dirty cache (CONSTITUTION §1.4.8): `is_dirty` reads
    /// ONLY this field. Recomputed in `update`, and ONLY there, at exactly
    /// two trigger points — see `recompute_dirty`'s doc comment.
    is_dirty_cached: bool,
    pub status_message: Option<String>,
    /// Provenance of `status_message` — see `StatusSource`'s docs. Only
    /// meaningful while `status_message.is_some()`; a later `set_status`
    /// call always updates both fields together, so a stale value here
    /// after the message is cleared can never be observed.
    pub status_source: StatusSource,
    /// This session's recovery store (plan WP5) — `None` only when no
    /// store could be constructed at all (an extreme fallback distinct from
    /// `AppDb::degraded`, which still has a live, if untrusted, store).
    pub db: Option<AppDb>,
    /// A persistent status banner independent of `status_message`'s
    /// provenance-cleared slot (plan WP5.S2/S3: "persistent status banner")
    /// — set once the store degrades (at open, or from a later
    /// `on_store_failure`) and never cleared automatically (WP5 has no
    /// store-reopen path).
    pub db_banner: Option<String>,
    /// The armed degraded-save confirm chord's timer generation — `None`
    /// when no confirm is pending (plan WP5.S2/S6, mirroring `pending_quit`
    /// below). Stale `SaveConfirmTimeout` generations are ignored.
    pub pending_save_confirm: Option<u32>,
    next_save_confirm_gen: u32,
    /// The armed quit chord and its timer generation — `None` when no quit
    /// is pending. Stale `ConfirmTimeout` generations are ignored (plan
    /// Context, "Quit-confirm").
    pub pending_quit: Option<(QuitKey, u32)>,
    next_quit_gen: u32,
    pub should_quit: bool,
    /// The most recent display-pipeline snapshot, cached by `sync_view` for
    /// `render::draw` to blit. `None` only before the first sync.
    pub view: Option<ViewSnapshots>,
}

impl App {
    pub fn new(
        buffer: Buffer,
        file_path: Option<PathBuf>,
        vfs: Arc<dyn Vfs + Send + Sync>,
        db: Option<AppDb>,
    ) -> App {
        let saved_version = buffer.version();
        App {
            editor: Editor::new(buffer),
            file_path,
            vfs,
            saved_version,
            save_pending_version: None,
            save_in_flight: false,
            is_dirty_cached: false,
            status_message: None,
            status_source: StatusSource::Other,
            db,
            db_banner: None,
            pending_save_confirm: None,
            next_save_confirm_gen: 0,
            pending_quit: None,
            next_quit_gen: 0,
            should_quit: false,
            view: None,
        }
    }

    /// Reads the render-only dirty cache — see `recompute_dirty`'s doc
    /// comment for the two points that keep it current (CONSTITUTION
    /// §1.4.8: dirty is recomputed only in `update`, never guessed at
    /// render time).
    pub fn is_dirty(&self) -> bool {
        self.is_dirty_cached
    }

    /// Marks the freshly constructed buffer dirty relative to the file it
    /// was hydrated from — for `rune-cli::main`'s bootstrap ONLY, called
    /// (at most once, before the runtime loop and thus before `update` has
    /// ever run) when `rune-db`'s `Load` ack reports `recovered !=
    /// disk_content`: pending journaled edits this session inherited were
    /// never actually written to disk. A direct fact from that ack, not a
    /// guess (§1.4.8's "baseline only ever from store acks") — this is the
    /// one place outside `update` allowed to touch the cache, precisely
    /// because there is no `update` call yet at this point in the program.
    pub fn mark_dirty_from_hydration(&mut self) {
        self.is_dirty_cached = true;
    }

    pub fn file_name(&self) -> &str {
        self.file_path
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("[No Name]")
    }

    /// Re-runs the display pipeline and caches the result for `render::draw`.
    /// Safe to call more than once per message batch — see `Editor::sync`'s
    /// docs.
    pub fn sync_view(&mut self) {
        self.view = Some(self.editor.sync());
    }

    /// The single writer of a NEW `status_message`: every call site that
    /// wants to set one goes through here instead of writing
    /// `status_message`/`status_source` separately, so the text and its
    /// provenance tag (`StatusSource`) can never drift apart.
    pub fn set_status(&mut self, message: impl Into<String>, source: StatusSource) {
        self.status_message = Some(message.into());
        self.status_source = source;
    }
}

/// The ONLY writer of `App` state (§5.4). `effects` accumulates I/O for the
/// runtime loop to perform after the whole message batch is applied:
/// `effects.raw` for OSC 52 (drained by the main loop, never a `Cmd` — plan
/// Gotchas, "Cmds must never touch the terminal"), `effects.cmds` for
/// off-thread work (save, pbpaste, the quit-confirm/save-confirm/snapshot
/// timers).
///
/// Wraps `update_inner` with the ONE chokepoint for the snapshot-autosave
/// debounce (plan WP5.S6): every message that mutates
/// `app.editor.journal` — typing, undo/redo, cut, paste, ... — funnels
/// through `commands::edit::commit_edit_batch`/`undo`/`redo`, so comparing
/// the journal position before and after `update_inner` catches all of them
/// uniformly, without threading a debounce call through every editing
/// command's call site individually.
pub fn update(app: &mut App, msg: Msg, effects: &mut Effects) {
    let journal_pos_before = app.editor.journal.pos();
    update_inner(app, msg, effects);
    if app.editor.journal.pos() != journal_pos_before {
        schedule_snapshot_debounce(app, effects);
    }
}

fn update_inner(app: &mut App, msg: Msg, effects: &mut Effects) {
    match msg {
        Msg::Key(key) => handle_key(app, key, effects),
        Msg::Resize(width, height) => {
            app.editor
                .viewport
                .set_size(width, height.saturating_sub(1));
        }
        Msg::Paste(text) => {
            // Bracketed paste and pbpaste (`Msg::ClipboardRead` below) both
            // funnel through the same `handle_paste_content` (plan Gotchas:
            // "Bracketed paste vs pbpaste double-paste" — never handle one
            // event twice, never insert through two different paths).
            clipboard::handle_paste_content(app, &text);
        }
        Msg::ClipboardRead(text) => {
            clipboard::handle_paste_content(app, &text);
        }
        Msg::SaveDone { version, result } => {
            app.save_in_flight = false;
            match result {
                Ok(()) => {
                    if version > app.saved_version {
                        app.saved_version = version;
                    }
                    // Provenance-aware clear (review finding F2): only a
                    // message THIS save path set (a prior failed/un-
                    // attempted save) is dismissed here. An unrelated
                    // message — a pbpaste failure, an edit/undo/redo
                    // failure — survives a successful save exactly as it
                    // already survives a successful edit (F2's original
                    // rule in `commands::edit`).
                    if app.status_source == StatusSource::SaveError {
                        app.status_message = None;
                    }
                }
                Err(e) => {
                    app.set_status(format!("save failed: {e}"), StatusSource::SaveError);
                }
            }
            recompute_dirty(app);
        }
        Msg::ConfirmTimeout { generation } => {
            if let Some((_, pending_gen)) = app.pending_quit
                && pending_gen == generation
            {
                app.pending_quit = None;
            }
            // A stale generation (the user already quit-confirmed or
            // re-armed with a new chord since) is ignored.
        }
        Msg::SaveConfirmTimeout { generation } => {
            if app.pending_save_confirm == Some(generation) {
                app.pending_save_confirm = None;
            }
        }
        Msg::SnapshotDue { generation } => handle_snapshot_due(app, generation),
        Msg::Db(evt) => handle_db_event(app, evt),
        Msg::Error(e) => {
            app.set_status(e, StatusSource::Other);
        }
        Msg::Quit => {
            app.should_quit = true;
        }
    }
}

/// A stale `generation` (a later journal mutation already rescheduled the
/// debounce — `schedule_snapshot_debounce`) is ignored. `content` and the
/// journal position ("current position", plan WP5.S6) are captured
/// SYNCHRONOUSLY here, in `update` — never re-derived once the enqueued
/// `CreateSnapshot` op actually runs on the writer thread (§1.4.2/§1.4.8's
/// "caller-captured, never re-derived" discipline, same as `materialize`).
/// Never touches the user's file — `create_snapshot` is a pure recovery
/// anchor (`rune-db::snapshot`'s doc comment).
fn handle_snapshot_due(app: &mut App, generation: u32) {
    let Some(db) = app.db.as_ref() else { return };
    if db.snapshot_generation != generation || db.degraded {
        return;
    }
    let content = app.editor.buffer.content().to_string();
    let result = db
        .store
        .create_snapshot(db.doc_id, &content, db.last_known_seq);
    if let Err(e) = result {
        on_store_failure(app, e.to_string());
    }
}

/// Bumps the snapshot-autosave generation and (re)schedules its 2s debounce
/// timer (plan WP5.S6, port of `workspace_timers.go:11`) — called once per
/// message batch that mutated the journal, from `update`'s wrapper above.
fn schedule_snapshot_debounce(app: &mut App, effects: &mut Effects) {
    let Some(db) = app.db.as_mut() else { return };
    db.snapshot_generation = db.snapshot_generation.wrapping_add(1);
    let generation = db.snapshot_generation;
    effects.cmds.push(snapshot_timeout_cmd(generation));
}

fn snapshot_timeout_cmd(generation: u32) -> Cmd {
    Cmd::new(CmdKind::SnapshotDebounce, move || {
        std::thread::sleep(SNAPSHOT_DEBOUNCE);
        Some(Msg::SnapshotDue { generation })
    })
}

/// Routes a `rune-db` writer-thread completion (plan WP5.S1). Only
/// `Materialize` acks (the save path, WP5.S6) and `AppendEdit` acks (seq
/// bookkeeping, `db::resolve_append_ack`) need a reaction on success;
/// `MoveUndoPos`/`CreateSnapshot`/adoption acks are fire-and-forget. Any
/// `Err`/`Fatal` degrades the store (plan decision 3) — never a buffer
/// rollback.
fn handle_db_event(app: &mut App, evt: DbEvent) {
    match evt {
        DbEvent::Ok {
            result: OpOutcome::Seq(seq),
            ..
        } => db::resolve_append_ack(app, seq),
        DbEvent::Ok {
            result: OpOutcome::Materialize(mat),
            ..
        } => handle_materialize_ack(app, *mat),
        DbEvent::Ok { .. } => {}
        DbEvent::Err { error, .. } => on_store_failure(app, error),
        DbEvent::Fatal { error } => on_store_failure(app, error),
    }
}

/// The reaction to a `materialize` ack (plan WP5.S6): advances
/// `saved_version`/`AppDb::expect_obs`/`bind_new` on a commit, surfaces each
/// `MatResult` outcome as status text, and — either way — clears
/// `save_in_flight` and recomputes the dirty cache (trigger (b) of
/// `recompute_dirty`'s doc comment).
fn handle_materialize_ack(app: &mut App, mat: MatResult) {
    app.save_in_flight = false;
    let pending_version = app.save_pending_version.take();

    if mat.committed {
        if let Some(saved) = &mat.saved
            && let Some(db) = app.db.as_mut()
        {
            db.expect_obs = saved.id;
            db.bind_new = false;
        }
        if let Some(version) = pending_version
            && version > app.saved_version
        {
            app.saved_version = version;
        }
        if app.status_source == StatusSource::SaveError {
            app.status_message = None;
        }
        if mat.raced {
            app.set_status(
                "saved \u{2014} a concurrent external change was overwritten; its bytes were preserved",
                StatusSource::Other,
            );
        }
    } else if mat.missing {
        app.set_status(
            "save failed: file no longer exists",
            StatusSource::SaveError,
        );
    } else {
        app.set_status(
            "save refused \u{2014} the file changed on disk since it was opened",
            StatusSource::SaveError,
        );
    }
    recompute_dirty(app);
}

/// A store enqueue-time error or an async `DbEvent::Err`/`Fatal` landed
/// (plan decision 3): the in-memory buffer/journal are NEVER rolled back —
/// only the store is marked degraded (sticky; WP5 has no reopen path) and a
/// persistent banner is raised. If a save was in flight, its guard is
/// released and the failure surfaces as an ordinary save error too, so
/// `trigger_save`'s in-flight guard can never wedge open on a lost ack.
pub(crate) fn on_store_failure(app: &mut App, error: String) {
    if let Some(db) = app.db.as_mut() {
        db.degraded = true;
    }
    app.db_banner = Some(format!("recovery disabled: {error}"));
    if app.save_in_flight {
        app.save_in_flight = false;
        app.save_pending_version = None;
        app.set_status(format!("save failed: {error}"), StatusSource::SaveError);
    }
}

/// CONSTITUTION §1.4.8: `is_dirty` reads only the cache this recomputes —
/// called from `update` at exactly two trigger points: (a) any journal
/// mutation (`commands::edit::commit_edit_batch`/`undo`/`redo`, immediately
/// after they mutate `app.editor.journal`) and (b) any `DbEvent` ack that
/// moves the baseline (`handle_materialize_ack`, immediately after
/// `saved_version` itself changes). The comparison is the buffer-version
/// proxy already established pre-WP5 (`saved_version`, now advanced ONLY by
/// a successful materialize ack or the no-store fallback's `SaveDone`) —
/// both trigger points call this immediately after whichever of
/// `saved_version`/`buffer.version()` they just changed, so the cache never
/// observes a stale pairing.
pub(crate) fn recompute_dirty(app: &mut App) {
    app.is_dirty_cached = app.editor.buffer.version() != app.saved_version;
}

fn handle_key(app: &mut App, key: KeyInput, effects: &mut Effects) {
    // Hardcoded fast paths outside the resolver, exactly as Go
    // (`textedit/update.go:67-85`): Enter (mod 0) -> newline; Escape ->
    // collapse selection. Neither is a resolver-bound chord (plan Context,
    // "Keymap").
    if key.code == KeyCode::Enter && key.mods == Mods::NONE {
        edit::newline(app);
        return;
    }
    if key.code == KeyCode::Escape && key.mods == Mods::NONE {
        nav::escape(app);
        return;
    }

    let Some(command) = keymap::resolve(key) else {
        // Unmatched printable text -> insert fallthrough (plan Context,
        // "Hardcoded fast paths outside the resolver": `update.go:134-158`).
        // Ctrl/Alt/Super chords that reach here are simply unbound, never an
        // insert — every bound Ctrl/Alt/Super chord is already caught by
        // `keymap::resolve` above.
        if let KeyCode::Char(ch) = key.code
            && !key.mods.ctrl
            && !key.mods.alt
            && !key.mods.sup
            && is_insertable_key_char(ch)
        {
            edit::insert_char(app, ch);
        }
        return;
    };

    match command {
        Command::CharLeft => nav::char_left(app, false),
        Command::CharRight => nav::char_right(app, false),
        Command::LineUp => nav::line_up(app, false),
        Command::LineDown => nav::line_down(app, false),
        Command::WordLeft => nav::word_left(app, false),
        Command::WordRight => nav::word_right(app, false),
        Command::LineStart => nav::line_start(app, false),
        Command::LineEnd => nav::line_end(app, false),
        Command::PageUp => nav::page_up(app, false),
        Command::PageDown => nav::page_down(app, false),
        Command::SelectCharLeft => nav::char_left(app, true),
        Command::SelectCharRight => nav::char_right(app, true),
        Command::SelectLineUp => nav::line_up(app, true),
        Command::SelectLineDown => nav::line_down(app, true),
        Command::SelectWordLeft => nav::word_left(app, true),
        Command::SelectWordRight => nav::word_right(app, true),
        Command::SelectLineStart => nav::line_start(app, true),
        Command::SelectLineEnd => nav::line_end(app, true),
        Command::SelectPageUp => nav::page_up(app, true),
        Command::SelectPageDown => nav::page_down(app, true),
        Command::SelectAll => nav::select_all(app),
        Command::DeleteLeft => edit::delete_left(app),
        Command::DeleteRight => edit::delete_right(app),
        Command::Indent => edit::indent(app),
        Command::Outdent => edit::outdent(app),
        Command::Undo => edit::undo(app),
        Command::Redo => edit::redo(app),
        Command::Copy => clipboard::copy(app, effects),
        Command::Cut => clipboard::cut(app, effects),
        Command::Paste => clipboard::paste(effects),
        Command::Save => trigger_save(app, effects),
        Command::QuitConfirm => {
            // `resolve` only ever returns `QuitConfirm` when `key` is a
            // known quit chord (see `keymap::QuitKey::from_key`, the single
            // source of truth both functions route through).
            if let Some(quit_key) = QuitKey::from_key(key) {
                handle_quit_key(app, quit_key, effects);
            }
        }
    }
}

/// Guards the printable-insert fallthrough against control-byte leakage
/// (data-integrity fix, review finding F1). Go's equivalent gate is
/// `isPrintableChar` (`textedit.go:441-443`: `r >= ' ' && r <= '~'`), but
/// that gate applies ONLY to Go's SYNTHESIZED-from-`BaseCode` case
/// (`update.go:136-145`) — real decoded text (`msg.Text`, and everything
/// `Msg::Paste` carries here) flows unrestricted, including non-ASCII
/// (CJK, emoji). This crate's termina-backed `KeyCode::Char(char)` has no
/// such split: it is Go's `BaseCode` concept alone, never a separate
/// decoded-text stream, so a literal ASCII-only port would also block
/// genuine direct-keystroke Unicode entry Go itself allows unrestricted
/// (and which `tests/tui_edit.rs` requires). The hazard Go's gate actually
/// closes is narrower than "ASCII only": a raw C0 control byte or DEL
/// leaking through as `Char` with no modifier flag at all — the reported
/// case is a non-Kitty terminal's legacy encoding, where Ctrl+A IS the
/// literal SOH byte (no separate "this was a chord" signal survives
/// decoding) rather than a Kitty-protocol key report with an explicit
/// Ctrl modifier. Such a leaked byte can only ever be a single codepoint
/// in `0x00..=0x1F` or `0x7F` — ASCII's own control range — so excluding
/// `char::is_control()` (Unicode category Cc: `0x00..=0x1F` and
/// `0x7F..=0x9F`) closes that exact hazard without narrowing what a human
/// can actually type.
fn is_insertable_key_char(ch: char) -> bool {
    !ch.is_control()
}

/// Port of the quit-confirm state machine (plan Context, "Quit-confirm",
/// mirroring `footer.go:230-237`): the SAME chord pressed twice quits;
/// pressing a quit chord while a DIFFERENT quit chord is pending re-arms
/// with the new chord and a fresh generation, restarting the 2s window.
fn handle_quit_key(app: &mut App, key: QuitKey, effects: &mut Effects) {
    if let Some((pending_key, generation)) = app.pending_quit
        && pending_key == key
    {
        let _ = generation; // the SAME chord always quits regardless of generation
        app.should_quit = true;
        return;
    }

    let generation = app.next_quit_gen;
    app.next_quit_gen = app.next_quit_gen.wrapping_add(1);
    app.pending_quit = Some((key, generation));
    effects.cmds.push(quit_confirm_timeout_cmd(generation));
}

/// The 2s quit-confirm timer, carrying its generation so a stale timeout
/// (superseded by a second press or a re-arm) is ignored on arrival (plan
/// Context, "Quit-confirm"). Genuine wall-clock pacing for a real UI
/// feature — not a test-ordering hack — so `std::thread::sleep` here is the
/// correct primitive (this Cmd runs on its own dedicated thread by runtime
/// design, never blocking the main loop).
fn quit_confirm_timeout_cmd(generation: u32) -> Cmd {
    Cmd::new(CmdKind::QuitTimeout, move || {
        std::thread::sleep(CONFIRM_TIMEOUT);
        Some(Msg::ConfirmTimeout { generation })
    })
}

/// `super+s` (WP9, plan Context "Save"; WP5.S6 routes it through
/// `rune-db`'s `materialize` on the writer FIFO when a store is present).
/// Guarded by the in-flight flag (a second `super+s` before the first
/// save's ack reports back is a no-op) and by `version != saved_version`
/// (nothing to persist otherwise).
///
/// When the store is degraded (open-ladder fallback or a later
/// `on_store_failure`), the FIRST `super+s` only arms a confirm gate
/// (mirrors `handle_quit_key`'s pending_quit shape, plan WP5.S2: "confirm
/// gate before materialize") — a document with no durable recovery journal
/// can still be saved, but only once the user has explicitly acknowledged
/// that crash protection is off; a SECOND `super+s` within the window
/// proceeds.
///
/// With no store at all (an extreme fallback beyond even `degraded` —
/// Prime Directive: the user must always be able to save, plan decision 5:
/// "losing the DB never damages a user file"), falls back to the pre-WP5
/// direct `vfs.save_atomic` `Cmd`.
fn trigger_save(app: &mut App, effects: &mut Effects) {
    if app.save_in_flight {
        return;
    }
    let version = app.editor.buffer.version();
    if version == app.saved_version {
        return;
    }
    let Some(path) = app.file_path.clone() else {
        app.set_status(
            "no file to save \u{2014} rune was opened without a path",
            StatusSource::SaveError,
        );
        return;
    };

    let Some(db) = &app.db else {
        // No store at all — the pre-WP5 direct-vfs fallback.
        app.save_in_flight = true;
        let bytes = app.editor.buffer.content().as_bytes().to_vec();
        let vfs = Arc::clone(&app.vfs);
        effects.cmds.push(save_cmd(vfs, path, bytes, version));
        return;
    };

    if db.degraded {
        if let Some(generation) = app.pending_save_confirm {
            let _ = generation;
            app.pending_save_confirm = None;
            materialize_now(app, path, version);
        } else {
            let generation = app.next_save_confirm_gen;
            app.next_save_confirm_gen = app.next_save_confirm_gen.wrapping_add(1);
            app.pending_save_confirm = Some(generation);
            app.set_status(
                "recovery disabled \u{2014} press \u{2318}S again to save anyway",
                StatusSource::Other,
            );
            effects.cmds.push(save_confirm_timeout_cmd(generation));
        }
        return;
    }

    materialize_now(app, path, version);
}

/// Enqueues `content` to `rune-db`'s writer FIFO via `Store::materialize`
/// (plan WP5.S6). Not a `Cmd`: `Store::enqueue` is a plain, non-blocking
/// channel send (never I/O that leaves this thread — the actual disk write
/// happens on the writer thread, whose eventual `DbEvent::Ok{ result:
/// OpOutcome::Materialize(..), .. }` ack arrives as `Msg::Db`, handled by
/// `handle_materialize_ack`), so §5.4 lets `update` call it directly.
/// `content`/`expect`/`seq`/`bind_new` are all captured HERE, synchronously
/// — never re-derived once the op runs (§1.4.2/§1.4.8).
fn materialize_now(app: &mut App, path: PathBuf, version: u64) {
    let Some(db) = app.db.as_ref() else { return };
    let content = app.editor.buffer.content().to_string();
    let result = db.store.materialize(
        db.doc_id,
        &path,
        &content,
        db.expect_obs,
        db.last_known_seq,
        db.bind_new,
    );

    app.save_in_flight = true;
    app.save_pending_version = Some(version);
    if let Err(e) = result {
        // A store enqueue-time failure is exactly the same class of event
        // as an async `DbEvent::Err`/`Fatal` (plan decision 3) — degrade
        // the store and raise the sticky banner via the same chokepoint
        // `append_edit`/`move_undo_pos` use (`handle_snapshot_due`,
        // `db.rs`), not a one-shot `SaveError` status that leaves `db.
        // degraded` untouched and lets the next save silently retry
        // against an already-wedged writer.
        on_store_failure(app, e.to_string());
    }
}

/// The 2s degraded-save confirm-gate timer (plan WP5.S2/S6) — mirrors
/// `quit_confirm_timeout_cmd`'s shape exactly.
fn save_confirm_timeout_cmd(generation: u32) -> Cmd {
    Cmd::new(CmdKind::SaveConfirmTimeout, move || {
        std::thread::sleep(SAVE_CONFIRM_TIMEOUT);
        Some(Msg::SaveConfirmTimeout { generation })
    })
}

/// The off-thread save I/O itself: `vfs.save_atomic` (§1.4.1's durable
/// temp-write + atomic publish, or `Mem`'s test double) writes EXACTLY
/// `bytes` — §1.4.5 byte-verbatim, no normalization anywhere on this path.
/// Only reached when `App::db` is `None` — see `trigger_save`'s docs.
fn save_cmd(vfs: Arc<dyn Vfs + Send + Sync>, path: PathBuf, bytes: Vec<u8>, version: u64) -> Cmd {
    Cmd::new(CmdKind::Save, move || {
        let result = vfs.save_atomic(&path, &bytes).map_err(|e| e.to_string());
        Some(Msg::SaveDone { version, result })
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::keymap::{KeyCode, Mods};
    use rune_vfs::{Disk, Mem, Vfs};

    fn test_app() -> App {
        App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None)
    }

    fn key(code: KeyCode, mods: Mods) -> KeyInput {
        KeyInput { code, mods }
    }

    #[test]
    fn first_quit_press_arms_and_spawns_a_timer_cmd_without_quitting() {
        let mut app = test_app();
        let mut effects = Effects::default();
        let ctrl_c = key(
            KeyCode::Char('c'),
            Mods {
                ctrl: true,
                ..Mods::NONE
            },
        );

        update(&mut app, Msg::Key(ctrl_c), &mut effects);

        assert!(!app.should_quit);
        assert_eq!(app.pending_quit, Some((QuitKey::CtrlC, 0)));
        assert_eq!(effects.cmds.len(), 1);
    }

    #[test]
    fn same_chord_twice_quits() {
        let mut app = test_app();
        let ctrl_c = key(
            KeyCode::Char('c'),
            Mods {
                ctrl: true,
                ..Mods::NONE
            },
        );

        let mut effects = Effects::default();
        update(&mut app, Msg::Key(ctrl_c), &mut effects);
        assert!(!app.should_quit);

        let mut effects = Effects::default();
        update(&mut app, Msg::Key(ctrl_c), &mut effects);
        assert!(app.should_quit);
    }

    #[test]
    fn different_quit_chord_re_arms_instead_of_quitting() {
        let mut app = test_app();
        let ctrl_c = key(
            KeyCode::Char('c'),
            Mods {
                ctrl: true,
                ..Mods::NONE
            },
        );
        let ctrl_alt_d = key(
            KeyCode::Char('d'),
            Mods {
                ctrl: true,
                alt: true,
                ..Mods::NONE
            },
        );

        let mut effects = Effects::default();
        update(&mut app, Msg::Key(ctrl_c), &mut effects);
        assert_eq!(app.pending_quit, Some((QuitKey::CtrlC, 0)));

        let mut effects = Effects::default();
        update(&mut app, Msg::Key(ctrl_alt_d), &mut effects);
        assert!(!app.should_quit, "a different quit chord must not quit");
        assert_eq!(app.pending_quit, Some((QuitKey::CtrlAltD, 1)));
    }

    #[test]
    fn matching_confirm_timeout_clears_pending_quit() {
        let mut app = test_app();
        let ctrl_c = key(
            KeyCode::Char('c'),
            Mods {
                ctrl: true,
                ..Mods::NONE
            },
        );
        let mut effects = Effects::default();
        update(&mut app, Msg::Key(ctrl_c), &mut effects);
        assert_eq!(app.pending_quit, Some((QuitKey::CtrlC, 0)));

        let mut effects = Effects::default();
        update(
            &mut app,
            Msg::ConfirmTimeout { generation: 0 },
            &mut effects,
        );
        assert_eq!(app.pending_quit, None);
        assert!(!app.should_quit);
    }

    #[test]
    fn stale_confirm_timeout_is_ignored() {
        let mut app = test_app();
        let ctrl_c = key(
            KeyCode::Char('c'),
            Mods {
                ctrl: true,
                ..Mods::NONE
            },
        );
        let ctrl_alt_d = key(
            KeyCode::Char('d'),
            Mods {
                ctrl: true,
                alt: true,
                ..Mods::NONE
            },
        );
        let mut effects = Effects::default();
        update(&mut app, Msg::Key(ctrl_c), &mut effects); // generation 0
        let mut effects2 = Effects::default();
        update(&mut app, Msg::Key(ctrl_alt_d), &mut effects2); // re-arms, generation 1
        assert_eq!(app.pending_quit, Some((QuitKey::CtrlAltD, 1)));

        // The stale generation-0 timeout must not clear the generation-1 pending quit.
        let mut effects3 = Effects::default();
        update(
            &mut app,
            Msg::ConfirmTimeout { generation: 0 },
            &mut effects3,
        );
        assert_eq!(app.pending_quit, Some((QuitKey::CtrlAltD, 1)));
    }

    #[test]
    fn save_done_ok_advances_saved_version_and_clears_a_prior_save_failure() {
        let mut app = test_app();
        let version = app.editor.buffer.version();

        // A real prior save failure — the only kind of message the
        // provenance-aware clear below (review finding F2) is allowed to
        // dismiss.
        let mut effects = Effects::default();
        update(
            &mut app,
            Msg::SaveDone {
                version,
                result: Err("oops".to_string()),
            },
            &mut effects,
        );
        assert!(app.status_message.is_some());

        let mut effects2 = Effects::default();
        update(
            &mut app,
            Msg::SaveDone {
                version,
                result: Ok(()),
            },
            &mut effects2,
        );
        assert_eq!(app.saved_version, version);
        assert!(
            app.status_message.is_none(),
            "a successful save must clear the failure message ITS OWN save path set"
        );
    }

    /// Regression for F2: a successful save must not clear a status message
    /// some OTHER subsystem set — e.g. an unresolved `Msg::Error` such as a
    /// pbpaste failure the user hasn't dismissed yet. Only a message tagged
    /// `StatusSource::SaveError` (see `save_done_ok_advances_saved_version_
    /// and_clears_a_prior_save_failure`) may be cleared by `SaveDone { Ok
    /// }`.
    #[test]
    fn save_done_ok_does_not_clear_an_unrelated_status_message() {
        let mut app = test_app();
        let mut effects = Effects::default();
        update(
            &mut app,
            Msg::Error("pbpaste failed to run: No such file or directory".to_string()),
            &mut effects,
        );
        assert!(app.status_message.is_some());

        let version = app.editor.buffer.version();
        let mut effects2 = Effects::default();
        update(
            &mut app,
            Msg::SaveDone {
                version,
                result: Ok(()),
            },
            &mut effects2,
        );

        assert_eq!(app.saved_version, version);
        assert!(
            app.status_message.is_some(),
            "a successful save must not clear an unrelated (non-save) status message"
        );
        assert!(
            app.status_message
                .as_deref()
                .is_some_and(|s| s.contains("pbpaste"))
        );
    }

    #[test]
    fn save_done_err_surfaces_status_and_keeps_dirty() {
        let mut app = test_app();
        app.editor.buffer = app.editor.buffer.insert(0, "x");
        let before_saved = app.saved_version;
        let version = app.editor.buffer.version();
        let mut effects = Effects::default();
        update(
            &mut app,
            Msg::SaveDone {
                version,
                result: Err("disk full".to_string()),
            },
            &mut effects,
        );
        assert_eq!(app.saved_version, before_saved);
        assert!(app.is_dirty());
        assert!(
            app.status_message
                .as_deref()
                .is_some_and(|s| s.contains("disk full"))
        );
    }

    #[test]
    fn resize_sets_viewport_size_reserving_the_status_row() {
        let mut app = test_app();
        let mut effects = Effects::default();
        update(&mut app, Msg::Resize(80, 24), &mut effects);
        assert_eq!(app.editor.viewport.width, 80);
        assert_eq!(app.editor.viewport.height, 23);
    }

    /// Regression for F1: a raw C0 control byte or DEL arriving as
    /// `KeyCode::Char` with NO modifier flag at all (the non-Kitty legacy-
    /// terminal degradation path, where Ctrl+A IS the literal SOH byte)
    /// must never reach the buffer.
    #[test]
    fn control_bytes_with_no_modifier_are_never_inserted() {
        let mut app = test_app();
        let before = app.editor.buffer.content().to_string();

        for raw in ['\u{1}', '\u{7f}', '\u{1b}'] {
            let mut effects = Effects::default();
            update(
                &mut app,
                Msg::Key(key(KeyCode::Char(raw), Mods::NONE)),
                &mut effects,
            );
        }

        assert_eq!(
            app.editor.buffer.content(),
            before,
            "a raw control byte must never be inserted into the document"
        );
    }

    #[test]
    fn printable_ascii_and_unicode_chars_are_still_insertable() {
        let mut app = test_app();
        let mut effects = Effects::default();
        update(
            &mut app,
            Msg::Key(key(KeyCode::Char('汉'), Mods::NONE)),
            &mut effects,
        );
        assert!(
            app.editor.buffer.content().contains('汉'),
            "genuine Unicode text entry must not be blocked by the control-byte guard"
        );
    }

    fn save_key() -> KeyInput {
        key(
            KeyCode::Char('s'),
            Mods {
                sup: true,
                ..Mods::NONE
            },
        )
    }

    /// Presses `super+s` through the real `update` and returns the
    /// `Effects` it produced — the caller drives `effects.cmds` to
    /// completion itself via `settle_cmds` (headless: this crate's `Cmd` is
    /// a plain `FnOnce`, no real thread or terminal needed to run one).
    fn press_save(app: &mut App) -> Effects {
        let mut effects = Effects::default();
        update(app, Msg::Key(save_key()), &mut effects);
        effects
    }

    /// Runs every `Cmd` in `effects` synchronously and feeds each resulting
    /// `Msg` back through `update`, recursively settling whatever new
    /// `Effects` that produces — the headless stand-in for `runtime::run`'s
    /// spawn-then-`recv` loop.
    fn settle_cmds(app: &mut App, effects: Effects) {
        for cmd in effects.cmds {
            if let Some(msg) = cmd.run() {
                let mut next = Effects::default();
                update(app, msg, &mut next);
                settle_cmds(app, next);
            }
        }
    }

    #[test]
    fn save_persists_exact_bytes_for_crlf_bom_and_no_trailing_newline_fixtures() {
        for content in ["a\r\nb\r\n", "\u{feff}hello", "no trailing newline"] {
            let vfs = Arc::new(Mem::new());
            let path = PathBuf::from("/doc.md");
            let mut app = App::new(
                Buffer::new(content),
                Some(path.clone()),
                Arc::clone(&vfs) as Arc<dyn Vfs + Send + Sync>,
                None,
            );
            // The buffer as freshly loaded IS the saved state (App::new sets
            // `saved_version = buffer.version()`) — force it dirty without
            // touching the CONTENT, so `super+s` actually has something to
            // persist and the assertion below is exercising the real write
            // path, not a same-content no-op.
            app.saved_version = 0;

            let effects = press_save(&mut app);
            assert_eq!(effects.cmds.len(), 1, "one save Cmd must be spawned");
            settle_cmds(&mut app, effects);

            let saved = vfs.read(&path).expect("save must have written the file");
            assert_eq!(
                saved,
                content.as_bytes(),
                "saved bytes must be byte-identical to the buffer, verbatim"
            );
            assert!(!app.is_dirty());
        }
    }

    #[test]
    fn save_failure_surfaces_a_status_error_and_keeps_dirty() {
        let vfs = Arc::new(Mem::new());
        vfs.fail_next_save(std::io::ErrorKind::Other);
        let path = PathBuf::from("/doc.md");
        let mut app = App::new(
            Buffer::new("hello"),
            Some(path),
            Arc::clone(&vfs) as Arc<dyn Vfs + Send + Sync>,
            None,
        );
        app.saved_version = 0; // force dirty — see the byte-exact test's comment

        let effects = press_save(&mut app);
        settle_cmds(&mut app, effects);

        assert!(app.is_dirty());
        assert!(
            app.status_message.is_some(),
            "a failed save must surface a status-line error"
        );
    }

    #[test]
    fn a_second_save_press_while_one_is_in_flight_is_a_no_op() {
        let mut app = App::new(
            Buffer::new("hello"),
            Some(PathBuf::from("/doc.md")),
            Arc::new(Mem::new()),
            None,
        );
        app.editor.buffer = app.editor.buffer.insert(0, "x"); // makes it dirty

        let effects = press_save(&mut app);
        assert_eq!(effects.cmds.len(), 1);
        assert!(app.save_in_flight);

        // A second press before the first save's Cmd has run must not spawn
        // a second Cmd.
        let effects2 = press_save(&mut app);
        assert!(
            effects2.cmds.is_empty(),
            "a save already in flight must not spawn a second save Cmd"
        );
        assert!(app.save_in_flight);
    }

    #[test]
    fn an_edit_during_a_save_keeps_the_buffer_dirty_once_the_save_completes() {
        let vfs = Arc::new(Mem::new());
        let path = PathBuf::from("/doc.md");
        let mut app = App::new(
            Buffer::new("hello"),
            Some(path),
            Arc::clone(&vfs) as Arc<dyn Vfs + Send + Sync>,
            None,
        );
        app.saved_version = 0; // force dirty — see the byte-exact test's comment

        let effects = press_save(&mut app); // captures the pre-edit version
        assert_eq!(effects.cmds.len(), 1);

        // An edit lands while the save Cmd hasn't reported back yet.
        edit::insert_char(&mut app, '!');
        let after_edit_version = app.editor.buffer.version();

        settle_cmds(&mut app, effects); // delivers SaveDone for the OLD version

        assert!(
            app.saved_version < after_edit_version,
            "SaveDone must only advance saved_version to the version IT saved, \
             not the buffer's current (post-edit) version"
        );
        assert!(
            app.is_dirty(),
            "an edit made during the in-flight save must leave the buffer dirty \
             once that save completes"
        );
    }

    #[test]
    fn saving_a_path_that_does_not_exist_on_disk_creates_it_via_the_excl_path() {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("rune-wp9-excl-{}-{n}.md", std::process::id()));
        let _ = std::fs::remove_file(&path); // in case a prior run left it behind
        assert!(!path.exists(), "the fixture path must not exist yet");

        let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Disk);
        let mut app = App::new(
            Buffer::new("brand new file\n"),
            Some(path.clone()),
            vfs,
            None,
        );
        app.saved_version = 0; // force dirty — see the byte-exact test's comment

        let effects = press_save(&mut app);
        settle_cmds(&mut app, effects);

        assert!(!app.is_dirty());
        let saved = std::fs::read(&path).expect("save must have created the file on disk");
        assert_eq!(saved, b"brand new file\n");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn every_cmd_is_tagged_with_its_kind() {
        let vfs = Arc::new(Mem::new());
        let mut app = App::new(
            Buffer::new("x"),
            Some(PathBuf::from("/doc.md")),
            Arc::clone(&vfs) as Arc<dyn Vfs + Send + Sync>,
            None,
        );
        app.saved_version = 0; // force dirty without touching content (see above)
        let effects = press_save(&mut app);
        assert_eq!(effects.cmds.len(), 1);
        assert_eq!(effects.cmds[0].kind(), CmdKind::Save);

        let mut app2 = test_app();
        let mut e2 = Effects::default();
        update(
            &mut app2,
            Msg::Key(key(
                KeyCode::Char('c'),
                Mods {
                    ctrl: true,
                    ..Mods::NONE
                },
            )),
            &mut e2,
        );
        assert_eq!(e2.cmds.len(), 1);
        assert_eq!(e2.cmds[0].kind(), CmdKind::QuitTimeout);

        let mut e3 = Effects::default();
        crate::commands::clipboard::paste(&mut e3);
        assert_eq!(e3.cmds.len(), 1);
        assert_eq!(e3.cmds[0].kind(), CmdKind::ClipboardRead);
    }
}
