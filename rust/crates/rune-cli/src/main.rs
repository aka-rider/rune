//! `rune`: the CLI entry point (plan Context, WP5.S2). Parses one positional
//! file (abs-path'd) or `--version`; a nonexistent path opens an empty
//! buffer (created on first save via `RENAME_EXCL`); invalid UTF-8 is
//! refused at load, before the TUI is ever entered (CONSTITUTION §0, plan
//! decision 4); a panic anywhere in the run loop is caught here, after the
//! terminal has already been restored by `term::Guard`'s `Drop` running
//! during unwind.

use std::any::Any;
use std::env;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use rune_core::buffer::{AppliedEdit, Buffer, BufferError};
use rune_core::undo::Step;
use rune_db::{DbEvent, OpOutcome, Store};
use rune_tui::app::App;
use rune_tui::db::{AppDb, DbBridge};
use rune_vfs::{Disk, Vfs};

/// `sysexits.h`-flavored exit codes: `EX_USAGE` (bad invocation), `EX_DATAERR`
/// (the file's bytes are not valid data for this program — invalid UTF-8),
/// `EX_IOERR` (the file exists but couldn't be read), `EX_SOFTWARE` (an
/// internal error — a recovered panic or a runtime I/O failure).
mod exit_code {
    pub const USAGE: u8 = 64;
    pub const DATA_ERR: u8 = 65;
    pub const IO_ERR: u8 = 74;
    pub const SOFTWARE: u8 = 70;
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();

    if args.iter().any(|a| a == "--version") {
        println!("rune {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }

    let Some(path_arg) = args.first() else {
        eprintln!("usage: rune <file.md>");
        return ExitCode::from(exit_code::USAGE);
    };

    let path = to_abs_path(path_arg);

    // Constructed before the load so the whole load path — like every other
    // filesystem access in this app (CONSTITUTION §1.4.9) — goes through the
    // injected `Vfs`, not a direct `std::fs` call: see `load_buffer`.
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Disk);

    let file_existed = vfs.stat(&path).is_ok();

    let buffer = match load_buffer(vfs.as_ref(), &path) {
        Ok(buffer) => buffer,
        Err(LoadError::InvalidUtf8) => {
            eprintln!(
                "rune: {} is not valid UTF-8 — refusing to open (file left untouched)",
                path.display()
            );
            return ExitCode::from(exit_code::DATA_ERR);
        }
        Err(LoadError::Io(e)) => {
            eprintln!("rune: failed to read {}: {e}", path.display());
            return ExitCode::from(exit_code::IO_ERR);
        }
    };

    // The recovery store (plan WP5.S2/S4). `rune_db::load` itself requires
    // the target to already exist on disk (`vfs.resolve`+`vfs.read` with no
    // NotFound-tolerant branch, unlike `load_buffer` above) — a brand-new
    // document has no `documents` row to bind yet (WP4 deliberately left
    // "create a scratch/untitled document" out of scope, `document.rs`'s
    // module doc), so hydration is skipped entirely for that case: the
    // editor still opens and runs fully, just without recovery journaling
    // for THIS launch. Any hydration failure is non-fatal for the same
    // reason (CONSTITUTION Prime Directive: protect the user's words over
    // every other feature) — it is reported to stderr, not to the TUI
    // (which hasn't started yet), and the editor proceeds with `app.db =
    // None`.
    let db_bootstrap = if file_existed {
        bootstrap_db(Arc::clone(&vfs), &path)
    } else {
        DbBootstrap::default()
    };

    let buffer = match &db_bootstrap.recovered_content {
        Some(content) => Buffer::new(content.clone()),
        None => buffer,
    };

    let mut app = App::new(buffer, Some(path), vfs, db_bootstrap.app_db);
    app.db_banner = db_bootstrap.banner;
    if let Some(bridge_edit) = db_bootstrap.bridge_edit {
        // Seeds the LOCAL in-memory undo journal with the ONE synthetic
        // edit `rune_db::load` itself already journaled durably (its own
        // `find_inheritable_draft` bridge, `load.rs`'s module doc) when
        // this session inherited a dead session's unsaved content — so
        // post-restart undo reaches the anchor (plan WP5.S4) in exactly one
        // step, reverting straight back to `disk_content`. Pushed directly
        // (never through `commands::edit::commit_edit_batch`/`db::
        // append_edit`) — the durable side already has this edit recorded;
        // re-enqueueing it here would duplicate it.
        app.editor.journal.push(Step {
            edits: vec![bridge_edit],
            cursors_before: Vec::new(),
            cursors_after: Vec::new(),
        });
        // A direct fact from the `Load` op's own ack (`recovered !=
        // disk_content`) — not a guess (§1.4.8's "baseline only ever from
        // store acks").
        app.mark_dirty_from_hydration();
    }

    let result = panic::catch_unwind(AssertUnwindSafe(|| rune_tui::runtime::run(&mut app)));

    // Drain the writer FIFO and run clean-shutdown housekeeping
    // (`wal_checkpoint(TRUNCATE)`/`optimize`, WP6.S2) on EVERY exit path —
    // normal quit, a surfaced runtime error, AND a recovered panic — never
    // only the three bootstrap-failure branches above. Queued ops (an
    // in-flight `AppendEdit` burst, a pending snapshot, an in-flight
    // `Materialize`) would otherwise be silently abandoned when `main`
    // returns: a durability hole `Store::shutdown`'s own deterministic
    // drain exists specifically to close.
    if let Some(app_db) = app.db.take() {
        app_db.shutdown();
    }

    match result {
        Ok(Ok(())) => ExitCode::SUCCESS,
        Ok(Err(e)) => {
            eprintln!("rune: {e}");
            ExitCode::from(exit_code::SOFTWARE)
        }
        Err(payload) => {
            // The terminal is already restored: `term::Guard::drop` ran
            // while this panic unwound through `runtime::run`, before it
            // reached this `catch_unwind` boundary. The default panic hook
            // already wrote its own message to stderr, but that happened
            // WHILE the alternate screen was still active — the Guard's
            // restore-to-main-screen right after effectively erased it from
            // view. Print the payload again now, after restoration, so the
            // user actually sees why the process is exiting.
            eprintln!(
                "rune: internal error (recovered): {}",
                panic_message(&payload)
            );
            ExitCode::from(exit_code::SOFTWARE)
        }
    }
}

/// Extracts a human-readable message from a `catch_unwind` payload. `panic!`
/// with a string literal or `format!` produces `&'static str` or `String`
/// respectively — the two shapes the standard panic machinery actually
/// produces; anything else (a custom payload from a dependency) falls back
/// to a fixed placeholder rather than guessing.
fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        return (*s).to_string();
    }
    if let Some(s) = payload.downcast_ref::<String>() {
        return s.clone();
    }
    "non-string panic payload".to_string()
}

#[derive(Debug)]
enum LoadError {
    InvalidUtf8,
    Io(std::io::Error),
}

/// A nonexistent path opens an empty buffer — it's created on first save via
/// `RENAME_EXCL` (plan Assumptions, A3). Any other read failure (permission
/// denied, a directory, ...) is fatal. Invalid UTF-8 is refused here, before
/// the TUI is ever entered. Reads through `vfs` (CONSTITUTION §1.4.9:
/// "Reach the filesystem only through the injected `vfs.FS`") rather than
/// `std::fs` directly, so this whole load path is exercisable against `Mem`
/// in tests, not just against a real disk.
fn load_buffer(vfs: &dyn Vfs, path: &Path) -> Result<Buffer, LoadError> {
    let bytes = match vfs.read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(LoadError::Io(e)),
    };
    Buffer::from_bytes(bytes).map_err(|e| match e {
        BufferError::InvalidUtf8 => LoadError::InvalidUtf8,
        // `from_bytes` only ever returns `InvalidUtf8` (see rune-core) — the
        // other `BufferError` variants come from `apply_edits`, never from
        // loading raw bytes. Still handled explicitly rather than assumed,
        // per CONSTITUTION §1.3 ("surface invalid input — no silent
        // fallback").
        other => LoadError::Io(std::io::Error::other(other.to_string())),
    })
}

/// The result of [`bootstrap_db`] — everything `main` needs to finish
/// constructing `App` with a hydrated recovery store (plan WP5.S2/S4).
#[derive(Default)]
struct DbBootstrap {
    app_db: Option<AppDb>,
    /// `Some` only when `rune-db`'s `Load` reconstructed content that
    /// differs from the buffer `load_buffer` already read straight off
    /// disk (a crash-recovered draft this session inherited) — `main`
    /// replaces the plain disk buffer with this one when present.
    recovered_content: Option<String>,
    /// The single synthetic whole-content-replace edit to seed the LOCAL
    /// undo journal with — `Some` exactly when `recovered_content` differs
    /// from what's on disk (the buffer should also open dirty, see
    /// `App::mark_dirty_from_hydration`). Reconstructed identically to
    /// `rune_db::load`'s own internal bridge edit (`disk_content` ->
    /// `recovered`, see `load.rs`'s module doc) purely from the two
    /// strings `LoadResult` already exposes, so post-restart undo reaches
    /// the anchor (plan WP5.S4) without any new `rune-db` API surface.
    bridge_edit: Option<AppliedEdit>,
    /// The persistent degraded-store status banner (plan WP5.S2), or
    /// `None` when the store opened clean.
    banner: Option<String>,
}

/// Opens the recovery store at `versioning::production_db_path()` and
/// hydrates `path` through it (plan WP5.S2/S4), BEFORE the TUI ever starts
/// (`runtime::run` hasn't been called yet — no `Sender<Msg>` exists; see
/// `db::DbBridge`'s doc comment for why hydration blocks on its OWN
/// receiver instead). Never fatal to the editor: any failure here is
/// reported to stderr and this returns `DbBootstrap::default()` — the
/// editor still opens and runs fully, just without recovery journaling for
/// this launch (CONSTITUTION Prime Directive: the user's words come before
/// every other feature, plan decision 5: "losing the DB never damages a
/// user file").
fn bootstrap_db(vfs: Arc<dyn Vfs + Send + Sync>, path: &Path) -> DbBootstrap {
    let Some(db_path) = rune_db::production_db_path() else {
        return DbBootstrap {
            banner: Some("recovery disabled: $HOME not set".to_string()),
            ..DbBootstrap::default()
        };
    };

    let (bridge, rx) = DbBridge::bootstrap();
    let (store, open_warning) = match Store::open(&db_path, Arc::clone(&vfs), bridge.on_event()) {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("rune: recovery store unavailable: {e}");
            return DbBootstrap {
                banner: Some(format!("recovery disabled: {e}")),
                ..DbBootstrap::default()
            };
        }
    };
    let degraded_at_open = store.degraded();

    let load_op_id = match store.load(path) {
        Ok(id) => id,
        Err(e) => {
            eprintln!("rune: recovery store load failed: {e}");
            store.shutdown();
            return DbBootstrap {
                banner: Some(format!("recovery disabled: {e}")),
                ..DbBootstrap::default()
            };
        }
    };

    // Blocks main() — there is no runtime loop yet to be blocked instead
    // (`db::DbBridge`'s doc comment). Any event for a DIFFERENT op id can't
    // arrive yet (this is the very first op this `Store` has been asked to
    // run), but the match stays defensive rather than assuming it.
    let load_outcome = loop {
        match rx.recv() {
            Ok(DbEvent::Ok { id, result }) if id == load_op_id => break Ok(result),
            Ok(DbEvent::Err { id, error }) if id == load_op_id => break Err(error),
            Ok(DbEvent::Fatal { error }) => break Err(error),
            Ok(_) => continue,
            Err(_) => break Err("recovery store writer thread is gone".to_string()),
        }
    };

    let load_result = match load_outcome {
        Ok(OpOutcome::Load(load_result)) => *load_result,
        Ok(_) => {
            eprintln!("rune: recovery store returned an unexpected reply to Load");
            store.shutdown();
            return DbBootstrap {
                banner: Some(
                    "recovery disabled: internal error: unexpected reply to Load".to_string(),
                ),
                ..DbBootstrap::default()
            };
        }
        Err(e) => {
            eprintln!("rune: recovery store load failed: {e}");
            store.shutdown();
            return DbBootstrap {
                banner: Some(format!("recovery disabled: {e}")),
                ..DbBootstrap::default()
            };
        }
    };

    // §1.7: `saved_obs` is `None` here only if `load` itself failed to
    // adopt anything for this session/doc pair — "should not occur" per
    // `LoadResult::saved_obs`'s own doc comment, but a `0` fallback would be
    // a fabricated `ObsId` (AUTOINCREMENT ids start at 1, so `0` is never a
    // real row) silently handed to every later CAS `materialize` as if it
    // were a genuine baseline. Treat it as the loud internal error it is —
    // degrade rather than fake a baseline no observation backs.
    let Some(expect_obs) = load_result.saved_obs else {
        eprintln!("rune: recovery store load did not adopt a saved_obs baseline");
        store.shutdown();
        return DbBootstrap {
            banner: Some(
                "recovery disabled: internal error: load did not adopt a saved_obs baseline"
                    .to_string(),
            ),
            ..DbBootstrap::default()
        };
    };

    let bridge_edit = (load_result.recovered != load_result.disk_content).then(|| AppliedEdit {
        start: 0,
        end: load_result.disk_content.len(),
        deleted: load_result.disk_content.clone(),
        insert: load_result.recovered.clone(),
    });
    let app_db = AppDb::new(
        store,
        bridge,
        load_result.doc_id,
        degraded_at_open,
        expect_obs,
        false, // bind_new: `file_existed` at the call site guarantees the target exists
        // last_known_seq: `load` may have already durably journaled a
        // cross-session-inheritance bridge edit under THIS session's own
        // id — `bridge_seq` is that edit's own seq when it happened, and
        // this session's true durable journal head either way (a fresh
        // session journals nothing else during `load`). `0` would silently
        // regress behind it for any `move_undo_pos`/`materialize` issued
        // before the first ordinary `AppendEdit` ack lands (finding 8).
        load_result.bridge_seq.unwrap_or(0),
    );

    let banner = if degraded_at_open {
        Some(open_warning.unwrap_or_else(|| rune_db::DEGRADED_WARNING.to_string()))
    } else {
        None
    };

    DbBootstrap {
        app_db: Some(app_db),
        recovered_content: Some(load_result.recovered),
        bridge_edit,
        banner,
    }
}

fn to_abs_path(input: &str) -> PathBuf {
    let path = PathBuf::from(input);
    if path.is_absolute() {
        return path;
    }
    match env::current_dir() {
        Ok(cwd) => cwd.join(path),
        Err(_) => path,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use rune_vfs::Mem;

    #[test]
    fn load_buffer_reads_existing_file_through_the_vfs() {
        let vfs = Mem::new();
        let path = Path::new("/doc.md");
        vfs.save_atomic(path, b"hello").expect("seed the mem vfs");

        let buffer = load_buffer(&vfs, path).expect("existing file should load");
        assert_eq!(buffer.content(), "hello");
    }

    #[test]
    fn load_buffer_opens_empty_for_a_nonexistent_path() {
        let vfs = Mem::new();
        let buffer = load_buffer(&vfs, Path::new("/missing.md")).expect("missing path opens empty");
        assert!(buffer.is_empty());
    }

    #[test]
    fn load_buffer_refuses_invalid_utf8() {
        let vfs = Mem::new();
        let path = Path::new("/bad.md");
        vfs.save_atomic(path, &[0xff, 0xfe])
            .expect("seed the mem vfs");

        let err = load_buffer(&vfs, path).expect_err("invalid utf-8 must error");
        assert!(matches!(err, LoadError::InvalidUtf8));
    }
}
