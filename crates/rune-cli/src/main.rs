//! `rune`: the CLI entry point (plan Context, WP5.S2; strict argument
//! parsing and multi-file launch added WP7; the `launch`/`bootstrap` seam
//! and its remaining fixes added WP4). `cli::parse` (abs-path'ing every
//! positional and `-w`'s value against the ONE `cwd` [`launch`] reads) hands
//! back `--version`/`--help`, a parsed [`cli::Cli`], or a rejected command
//! line; the first positional opens through the load path below exactly as
//! a single-file launch always has, every remaining one opens as its own
//! tab, and the first stays the active document. No file opens an empty
//! untitled document; a nonexistent path opens an empty buffer (created on
//! first save via `RENAME_EXCL`); invalid UTF-8 is refused at load, before
//! the TUI is ever entered (CONSTITUTION §0, plan decision 4); a panic
//! anywhere in the run loop is caught here, after the terminal has already
//! been restored by `term::Guard`'s `Drop` running during unwind.
//!
//! [`bootstrap`] does everything up to (but not including) the interactive
//! run loop and is the seam WP4.S1/S7 test against `Mem`: it returns
//! `Err(ExitCode)` for every early exit (`--version`/`--help`, a rejected
//! command line, a load failure) and `Ok(AppGuard)` — a fully wired app,
//! ready for `runtime::run` — otherwise. [`launch`] wraps it with the
//! interactive run loop and is what `main` calls with the real `Disk` vfs
//! and process environment.

use std::any::Any;
use std::env;
use std::ffi::OsString;
use std::ops::{Deref, DerefMut};
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use rune_core::buffer::{Buffer, BufferError};
use rune_db::{DbEvent, OpOutcome, Store};
use rune_tui::app::App;
use rune_tui::banner;
use rune_tui::db::{Db, DbBridge, DocDb};
use rune_tui::{workspace, workspaceroot};
use rune_vfs::{Disk, FileKind, Vfs};

use cli::{CliAction, CliError};

mod cli;

/// `sysexits.h`-flavored exit codes: `EX_USAGE` (a malformed command line —
/// an unrecognised flag, a missing `-w` value, or `-w` pointing somewhere
/// that isn't a directory), `EX_DATAERR` (the file's bytes are not valid
/// data for this program — invalid UTF-8), `EX_IOERR` (the file exists but
/// couldn't be read, or the current directory itself couldn't be read),
/// `EX_SOFTWARE` (an internal error — a recovered panic or a runtime I/O
/// failure).
mod exit_code {
    pub const USAGE: u8 = 64;
    pub const DATA_ERR: u8 = 65;
    pub const IO_ERR: u8 = 74;
    pub const SOFTWARE: u8 = 70;
}

fn main() -> ExitCode {
    // Read exactly once (plan WP4.S6/[rune-cli 8]): every other spot that
    // used to read `env::current_dir()` a second time (the `-w`-absent
    // workspace-root fallback) now reuses this value, and a failure here is
    // surfaced instead of the two divergent silent fallbacks
    // (`to_abs_path`'s bare path, `workspaceroot`'s `"."`) the review found.
    let cwd = match env::current_dir() {
        Ok(cwd) => cwd,
        Err(e) => {
            eprintln!("rune: failed to read the current directory: {e}");
            return ExitCode::from(exit_code::IO_ERR);
        }
    };

    // Constructed before the load so the whole load path — like every other
    // filesystem access in this app (CONSTITUTION §1.4.9) — goes through the
    // injected `Vfs`, not a direct `std::fs` call: see `load_buffer`.
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Disk);
    let home = env::var_os("HOME").map(PathBuf::from);

    launch(vfs, env::args_os().skip(1), cwd, home)
}

/// Everything `main` does after resolving `cwd`/`$HOME` and constructing the
/// real `vfs` — factored out so it's callable with a `Mem` vfs and canned
/// args/cwd/home in tests (plan WP4.S1/[rune-cli 13]).
fn launch(
    vfs: Arc<dyn Vfs + Send + Sync>,
    args: impl Iterator<Item = OsString>,
    cwd: PathBuf,
    home: Option<PathBuf>,
) -> ExitCode {
    let mut app = match bootstrap(vfs, args, cwd, home) {
        Ok(app) => app,
        Err(code) => return code,
    };

    // Install the real hardware probe (plan WP5.S8) — production is the
    // ONLY place `HidSpaceProbe` is installed; every test and the fuzzer
    // keep `App::new`'s inert `NullProbe` default.
    app.space_probe = Box::new(rune_tui::keystate::HidSpaceProbe);
    // Prime `leader_available`'s cache now, at startup, rather than letting
    // the first `space+x`/`e`/`t` press pay for it: `leader_available` is a
    // `OnceLock` (plan WP3.S3), so this one-shot check runs here instead of
    // on the user's first keystroke.
    let _ = rune_tui::keystate::leader_available();

    let result = panic::catch_unwind(AssertUnwindSafe(|| rune_tui::runtime::run(&mut app)));

    // `app` (an `AppGuard`) drains the writer FIFO and runs clean-shutdown
    // housekeeping (`wal_checkpoint(TRUNCATE)`/`optimize`, WP6.S2) in its
    // `Drop`, which now runs on EVERY exit path this function has — normal
    // return below, an early `?`-style return inside `bootstrap` after the
    // guard exists, AND a panic unwinding through this frame — not only the
    // three bootstrap-failure branches `bootstrap` itself handles before an
    // `AppGuard` exists at all (plan WP4.S3/[rune-cli 5]: a panic between
    // the store opening and this `catch_unwind` boundary used to skip the
    // drain the old comment here claimed was unconditional).
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

/// Wraps `App` so the recovery store's writer thread is always drained,
/// regardless of how this value stops being live — normal end of scope OR
/// a panic unwinding through it (plan WP4.S3/[rune-cli 5]). The sole writer
/// of `App.db = None` from this point on; nothing past [`bootstrap`] calls
/// `Db::shutdown` directly.
struct AppGuard(App);

impl Deref for AppGuard {
    type Target = App;
    fn deref(&self) -> &App {
        &self.0
    }
}

impl DerefMut for AppGuard {
    fn deref_mut(&mut self) -> &mut App {
        &mut self.0
    }
}

impl Drop for AppGuard {
    fn drop(&mut self) {
        if let Some(db) = self.0.db.take() {
            db.shutdown();
        }
    }
}

/// Parses `args`, opens every file, and wires the recovery store — every
/// early exit (`--version`/`--help`, a rejected command line, a load
/// failure) returns `Err` with its exit code already decided; the success
/// path returns `Ok` with a fully wired [`AppGuard`], not yet handed to
/// `runtime::run`. This split (plan WP4.S1/[rune-cli 13]) is what makes the
/// wiring testable against `Mem`: a test can call this directly and inspect
/// the returned `App` without ever starting the interactive run loop.
fn bootstrap(
    vfs: Arc<dyn Vfs + Send + Sync>,
    args: impl Iterator<Item = OsString>,
    cwd: PathBuf,
    home: Option<PathBuf>,
) -> Result<AppGuard, ExitCode> {
    let launch = match cli::parse(args, &cwd) {
        Ok(CliAction::Version) => {
            println!("rune {}", env!("CARGO_PKG_VERSION"));
            return Err(ExitCode::SUCCESS);
        }
        Ok(CliAction::Help) => {
            println!("{}", cli::USAGE_TEXT);
            return Err(ExitCode::SUCCESS);
        }
        Ok(CliAction::Run(launch)) => launch,
        Err(e) => return Err(usage_error(&e)),
    };

    // `-w`'s existence/directory-ness check is the documented §1.4.9
    // launch-bootstrap exception, same class as `workspaceroot::resolve`'s
    // own `read_dir` walk below (WP7.S4).
    if let Some(dir) = &launch.work_dir
        && let Err(e) = validate_work_dir(vfs.as_ref(), dir)
    {
        return Err(usage_error(&e));
    }

    let (app, db_bootstrap) = if let Some(path) = launch.files.first() {
        // The SAME resolution chokepoint every other open path (every
        // extra positional below, and the Explorer) already funnels
        // through — `workspace::resolve` — so the first positional can
        // never bind as an unresolved spelling that a later open of the
        // identical underlying file (a symlink, a `..` segment, a
        // duplicated absolute path) would fail to recognize as the same
        // document (plan [rune-cli 2]).
        let path = workspace::resolve(vfs.as_ref(), path);

        let file_existed = vfs.stat(&path).is_ok();

        let buffer = match load_buffer(vfs.as_ref(), &path) {
            Ok(buffer) => buffer,
            Err(LoadError::InvalidUtf8) => {
                eprintln!(
                    "rune: {} is not valid UTF-8 — refusing to open (file left untouched)",
                    path.display()
                );
                return Err(ExitCode::from(exit_code::DATA_ERR));
            }
            Err(LoadError::Io(e)) => {
                eprintln!("rune: failed to read {}: {e}", path.display());
                return Err(ExitCode::from(exit_code::IO_ERR));
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
        // None`. This launch mode is otherwise silent about running with zero
        // crash protection (plan [rune-cli 3]) — every OTHER way this session
        // can end up without a recovery journal (a degraded open ladder, a
        // failed `Load`) already sets `app.db_banner`, so this one does too,
        // rather than leaving the user with no indication at all.
        let mut db_bootstrap = if file_existed {
            bootstrap_db(Arc::clone(&vfs), &path, home.as_deref())
        } else {
            DbBootstrap {
                banner: Some(
                    "recovery disabled: this file doesn't exist yet — no crash protection until \
                     it's first saved"
                        .to_string(),
                ),
                ..DbBootstrap::default()
            }
        };

        // The buffer stays exactly what `load_buffer` read off disk here —
        // adopting `recovered_content` goes through the same hydration
        // chokepoint (`Document::hydrate`, plan WP5.S2) `db::handle_load_ack`
        // uses, below, once `App::new` exists to hold it. Pre-replacing the
        // buffer here (as this used to) would skip that chokepoint's §1.3
        // suspicion check entirely.
        let app = App::new(buffer, Some(path), Arc::clone(&vfs), db_bootstrap.db.take());
        (app, db_bootstrap)
    } else {
        // No positional files — open the default untitled document
        // (`App::new_untitled`). The Go implementation uses
        // `nextUntitledName` to pick the first "Untitled N" not already
        // used by open tabs; at startup there are none, so this is always
        // "Untitled 1". No file on disk means no recovery store to
        // hydrate either — see `App::new_untitled`'s own docs.
        (App::new_untitled(Arc::clone(&vfs)), DbBootstrap::default())
    };

    // From here on, a panic unwinding through this function (or `launch`
    // above it, before `catch_unwind`) still drains the writer thread —
    // see `AppGuard`'s own doc.
    let mut app = AppGuard(app);

    // `-w` wins outright; otherwise walk up from `cwd` (falling back to the
    // first file's parent) for a `.git`/`.obsidian` marker. Every actual
    // directory read during the walk goes through `app.vfs`.
    let root = match &launch.work_dir {
        Some(dir) => dir.clone(),
        None => workspaceroot::resolve(
            app.vfs.as_ref(),
            &cwd,
            home.as_deref(),
            launch.files.first().map(|p| p.as_path()),
        ),
    };
    app.set_root(root);

    // The first positional is already open (above) and stays the active,
    // displayed document (Go treats it as the awaited display document and
    // the rest as tabs) — every REMAINING file opens as its own tab through
    // the same path the Explorer uses (WP7.S6). A failure there reports
    // into the error banner instead of aborting startup; every failure
    // across the whole batch is accumulated and reported ONCE (plan
    // WP4.S6/[rune-cli 7]) rather than letting only the last one survive a
    // string of "the modal replaces on ties" overwrites.
    let first_doc_id = app.active;
    let mut open_errors: Vec<String> = Vec::new();
    for extra in launch.files.iter().skip(1) {
        if workspace::open_path(&mut app, extra).is_none()
            && let Some(text) = take_error_banner(&mut app)
        {
            open_errors.push(text);
        }
    }
    if launch.files.len() > 1 {
        workspace::switch_to(&mut app, first_doc_id);
    }
    if !open_errors.is_empty() {
        banner::report_error(&mut app, combine_open_errors(&open_errors));
    }

    app.db_banner = db_bootstrap.banner;
    // The DocDb half of the old combined AppDb (plan WP1 decision 5)
    // installs on the initial document — App::new only wires up the
    // app-wide store handle above.
    if let Some(doc_db) = db_bootstrap.doc_db {
        app.active_doc_mut().db = Some(doc_db);
    }
    if let Some(recovered) = db_bootstrap.recovered_content {
        // Adopts a dead session's inherited draft content through the same
        // chokepoint `db::handle_load_ack` uses for every later per-document
        // hydration (plan WP5.S2): the §1.3 destructive-reset suspicion
        // check, the synthetic bridge `Step` so post-restart undo reaches
        // `disk_content` in one step, and a refusal surfaced rather than
        // silently applied. The buffer here still holds exactly what
        // `load_buffer` read off disk, so it IS `disk_content`.
        let disk_content = app.active_doc_mut().buffer.content().to_string();
        if let rune_tui::document::Hydration::Refused(reason) =
            app.active_doc_mut().hydrate(&disk_content, &recovered)
        {
            app.set_status(
                format!("crash recovery: {reason}"),
                rune_tui::app::StatusSource::Other,
            );
        }
    }

    Ok(app)
}

/// Peeks the modal `workspace::open_path` just raised on a failed open and,
/// if it's an error banner, clears it and returns its rendered text — so
/// the caller can accumulate several failures into one combined banner
/// instead of letting each overwrite the last (plan WP4.S6/[rune-cli 7]).
/// Only ever sees `Modal::Error` in this bootstrap window: nothing raises a
/// `Guard` prompt before the interactive run loop starts.
fn take_error_banner(app: &mut App) -> Option<String> {
    let text = match &app.modal {
        Some(banner::Modal::Error(state)) => Some(state.doc.buffer.content().to_string()),
        _ => None,
    };
    if text.is_some() {
        banner::clear_modal(app);
    }
    text
}

/// Combines the accumulated per-file open failures into one banner body
/// (plan WP4.S6/[rune-cli 7]): a single failure's own text verbatim, or a
/// count-prefixed list when more than one file failed to open.
fn combine_open_errors(errors: &[String]) -> String {
    if let [only] = errors {
        return only.clone();
    }
    let mut combined = format!(
        "{} of the requested files could not be opened:\n",
        errors.len()
    );
    for err in errors {
        combined.push_str("- ");
        combined.push_str(err);
        combined.push('\n');
    }
    combined
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

/// The result of [`bootstrap_db`] — everything [`bootstrap`] needs to finish
/// constructing `App` with a hydrated recovery store (plan WP5.S2/S4,
/// re-split in WP1 alongside `AppDb` -> `Db`/`DocDb`, plan decision 5):
/// `db` wires onto `App` directly (`App::new`'s 4th argument); `doc_db`
/// installs on the initial document afterward, since `App::new` only knows
/// about the app-wide half.
#[derive(Default)]
struct DbBootstrap {
    db: Option<Db>,
    doc_db: Option<DocDb>,
    /// `Some` whenever `rune-db`'s `Load` returned reconstructed content
    /// (which may or may not differ from the buffer `load_buffer` already
    /// read straight off disk) — `main` runs this through the same
    /// `Document::hydrate` chokepoint `db::handle_load_ack` uses, once
    /// `App::new` exists to hold the result.
    recovered_content: Option<String>,
    /// The persistent degraded-store status banner (plan WP5.S2), or
    /// `None` when the store opened clean.
    banner: Option<String>,
}

/// One exit path for every "recovery store bootstrap failed after a `Store`
/// was actually opened" branch below (plan WP4.S5/[rune-cli 11] — these
/// used to be written out four times near-verbatim): prints the reason,
/// drains the writer thread, and returns the all-`None`/banner-set
/// bootstrap the editor runs with when recovery is unavailable.
fn degrade(store: Store, msg: impl Into<String>) -> DbBootstrap {
    let msg = msg.into();
    eprintln!("rune: recovery store degraded: {msg}");
    store.shutdown();
    DbBootstrap {
        banner: Some(format!("recovery disabled: {msg}")),
        ..DbBootstrap::default()
    }
}

/// Opens the recovery store at `$HOME/Library/Application Support/rune/
/// rune-v{SCHEMA_VERSION}.db` and hydrates `path` through it (plan
/// WP5.S2/S4), BEFORE the TUI ever starts (`runtime::run` hasn't been
/// called yet — no `Sender<Msg>` exists; see `db::DbBridge`'s doc comment
/// for why hydration blocks on the bridge's OWN buffer instead). Never
/// fatal to the editor: any failure here is reported to stderr and this
/// returns `DbBootstrap::default()` — the editor still opens and runs
/// fully, just without recovery journaling for this launch (CONSTITUTION
/// Prime Directive: the user's words come before every other feature, plan
/// decision 5: "losing the DB never damages a user file").
///
/// `home` is threaded in rather than read from `$HOME` directly (unlike
/// `rune_db::production_db_path`) so this whole path is exercisable
/// against a temp directory in tests (plan WP4.S1/S7) without touching the
/// real machine's recovery store.
fn bootstrap_db(vfs: Arc<dyn Vfs + Send + Sync>, path: &Path, home: Option<&Path>) -> DbBootstrap {
    let db_path = match home {
        Some(home) if !home.as_os_str().is_empty() => home
            .join("Library")
            .join("Application Support")
            .join("rune")
            .join(rune_db::db_file_name(rune_db::SCHEMA_VERSION)),
        _ => {
            return DbBootstrap {
                banner: Some("recovery disabled: $HOME not set".to_string()),
                ..DbBootstrap::default()
            };
        }
    };

    let bridge = DbBridge::bootstrap();
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
        Err(e) => return degrade(store, format!("load failed: {e}")),
    };

    // Blocks main() — there is no runtime loop yet to be blocked instead
    // (`db::DbBridge`'s doc comment). Any event for a DIFFERENT op id can't
    // arrive yet (this is the very first op this `Store` has been asked to
    // run) — the predicate stays defensive rather than assuming it, and
    // leaves any such event buffered for `attach` rather than consuming it.
    // The writer thread always posts a `Fatal` before parking on a panic
    // (`writer.rs`'s own guarantee), so there is no "sender disconnected"
    // case left to handle here the way an `mpsc::Receiver` would need to.
    let load_outcome = match bridge.wait_for_bootstrap_event(|evt| match evt {
        DbEvent::Ok { id, .. } | DbEvent::Err { id, .. } => *id == load_op_id,
        DbEvent::Fatal { .. } => true,
    }) {
        DbEvent::Ok { result, .. } => Ok(result),
        DbEvent::Err { error, .. } => Err(error),
        DbEvent::Fatal { error } => Err(error),
    };

    let load_result = match load_outcome {
        Ok(OpOutcome::Load(load_result)) => *load_result,
        Ok(_) => {
            return degrade(store, "internal error: unexpected reply to Load");
        }
        Err(e) => return degrade(store, format!("load failed: {e}")),
    };

    // §1.7: `saved_obs` is `None` here only if `load` itself failed to
    // adopt anything for this session/doc pair — "should not occur" per
    // `LoadResult::saved_obs`'s own doc comment, but a `0` fallback would be
    // a fabricated `ObsId` (AUTOINCREMENT ids start at 1, so `0` is never a
    // real row) silently handed to every later CAS `materialize` as if it
    // were a genuine baseline. Treat it as the loud internal error it is —
    // degrade rather than fake a baseline no observation backs.
    let Some(expect_obs) = load_result.saved_obs else {
        return degrade(
            store,
            "internal error: load did not adopt a saved_obs baseline",
        );
    };

    let db = Db::new(store, bridge, degraded_at_open);
    let doc_db = DocDb::new(
        load_result.doc_id,
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
        db: Some(db),
        doc_db: Some(doc_db),
        recovered_content: Some(load_result.recovered),
        banner,
    }
}

fn to_abs_path(input: &str, cwd: &Path) -> PathBuf {
    let path = PathBuf::from(input);
    if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    }
}

/// `-w`'s own validation (WP7.S4): `dir` (already absolutized by
/// `cli::parse`) must `stat` successfully as a directory. Distinguishes
/// WHY it didn't (plan WP4.S6/[rune-cli 9]) — a nonexistent path, an
/// existing non-directory, or some other `stat` failure (permission
/// denied, an I/O error) each get their own [`CliError`] instead of one
/// wildcard "not a directory" collapsing all three. Split out from
/// `bootstrap` so it's exercisable against `Mem` in tests, exactly like
/// `load_buffer` above.
fn validate_work_dir(vfs: &dyn Vfs, dir: &Path) -> Result<(), CliError> {
    match vfs.stat(dir) {
        Ok(stat) if stat.kind == FileKind::Dir => Ok(()),
        Ok(_) => Err(CliError::NotADirectory(dir.to_path_buf())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err(CliError::WorkDirNotFound(dir.to_path_buf()))
        }
        Err(e) => Err(CliError::WorkDirUnreadable(
            dir.to_path_buf(),
            e.to_string(),
        )),
    }
}

/// The one exit path for every [`CliError`] — from `cli::parse` itself or
/// from `validate_work_dir` afterward: the specific message, then
/// [`cli::USAGE_TEXT`], both to stderr (WP7.S3).
fn usage_error(e: &CliError) -> ExitCode {
    eprintln!("rune: {e}");
    eprintln!("{}", cli::USAGE_TEXT);
    ExitCode::from(exit_code::USAGE)
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

    #[test]
    fn validate_work_dir_rejects_a_regular_file() {
        let vfs = Mem::new();
        let path = Path::new("/not/a/dir.md");
        vfs.save_atomic(path, b"hi").expect("seed the mem vfs");

        let err = validate_work_dir(&vfs, path).expect_err("a regular file is not a directory");
        assert!(matches!(err, CliError::NotADirectory(p) if p == path));
    }

    #[test]
    fn validate_work_dir_distinguishes_a_missing_directory() {
        let vfs = Mem::new();
        let err = validate_work_dir(&vfs, Path::new("/nope"))
            .expect_err("a missing directory must error");
        assert!(matches!(err, CliError::WorkDirNotFound(p) if p == Path::new("/nope")));
    }

    /// A real, throwaway `$HOME` under the OS temp dir — `Store::open`
    /// talks to the sqlite file directly via `rusqlite`, bypassing the
    /// injected `vfs` entirely (same pattern `rune-db`'s own multiprocess
    /// tests use), so the recovery-store half of these launch tests needs
    /// a real directory even though every document byte lives in `Mem`.
    struct ScratchHome(PathBuf);

    impl ScratchHome {
        fn new(label: &str) -> ScratchHome {
            let dir = env::temp_dir().join(format!(
                "rune-cli-launch-{label}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or_default()
            ));
            std::fs::create_dir_all(&dir).expect("create scratch home");
            ScratchHome(dir)
        }
    }

    impl Drop for ScratchHome {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn launch_multi_file_enqueues_a_load_for_every_extra_tab() {
        let vfs = Mem::new();
        vfs.save_atomic(Path::new("/vault/a.md"), b"a")
            .expect("seed a.md");
        vfs.save_atomic(Path::new("/vault/b.md"), b"b")
            .expect("seed b.md");
        vfs.save_atomic(Path::new("/vault/c.md"), b"c")
            .expect("seed c.md");
        let home = ScratchHome::new("multi-file");

        let app = bootstrap(
            Arc::new(vfs),
            vec![
                OsString::from("/vault/a.md"),
                OsString::from("/vault/b.md"),
                OsString::from("/vault/c.md"),
            ]
            .into_iter(),
            PathBuf::from("/"),
            Some(home.0.clone()),
        )
        .expect("bootstrap should succeed");

        // The first file hydrates synchronously inside `bootstrap_db` and
        // is bound before `App::new` ever runs.
        assert_eq!(app.documents.len(), 3);
        assert!(app.doc(app.active).is_some_and(|d| d.db.is_some()));
        // The other two open through `workspace::open_path`'s async path
        // (plan [rune-cli 1]/WP3.S1): each one's `Load` must actually be
        // enqueued and tracked, not silently dropped the way a `Sink::
        // Bootstrap`-less bridge used to swallow it — `db_ops` is where
        // `db::load_document` records that at enqueue time, synchronously.
        assert_eq!(
            app.db_ops.len(),
            2,
            "every extra tab's Load must be tracked"
        );
    }

    #[test]
    fn launch_same_file_two_spellings_opens_one_document() {
        let vfs = Mem::new();
        vfs.save_atomic(Path::new("/vault/notes.md"), b"hi")
            .expect("seed notes.md");
        let home = ScratchHome::new("dedup");

        let app = bootstrap(
            Arc::new(vfs),
            vec![
                OsString::from("/vault/notes.md"),
                OsString::from("/vault/sub/../notes.md"),
            ]
            .into_iter(),
            PathBuf::from("/"),
            Some(home.0.clone()),
        )
        .expect("bootstrap should succeed");

        assert_eq!(
            app.documents.len(),
            1,
            "two spellings of the same file must resolve to one document"
        );
    }

    #[test]
    fn launch_nonexistent_path_sets_a_banner() {
        let vfs = Mem::new();

        let app = bootstrap(
            Arc::new(vfs),
            vec![OsString::from("/vault/missing.md")].into_iter(),
            PathBuf::from("/"),
            None,
        )
        .expect("bootstrap should succeed even with no recovery store");

        assert!(
            app.db_banner.is_some(),
            "a nonexistent-path launch must not run with zero indication of no crash protection"
        );
    }

    #[test]
    fn launch_empty_positional_is_rejected_before_any_open() {
        let vfs = Mem::new();

        let result = bootstrap(
            Arc::new(vfs),
            vec![OsString::from("")].into_iter(),
            PathBuf::from("/"),
            None,
        );
        assert!(
            result.is_err(),
            "an empty positional must be rejected at parse"
        );
    }
}
