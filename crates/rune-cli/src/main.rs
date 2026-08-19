//! `rune`: the CLI entry point. `cli::parse` (abs-path'ing every
//! positional and `-w`'s value against the ONE `cwd` [`launch`] reads) hands
//! back `--version`/`--help`, a parsed [`cli::Cli`], or a rejected command
//! line; the first positional opens through the load path below exactly as
//! a single-file launch always has, every remaining one opens as its own
//! tab, and the first stays the active document. No file opens an empty
//! untitled document; a nonexistent path opens an empty buffer (created on
//! first save via `RENAME_EXCL`); invalid UTF-8 is refused at load, before
//! the TUI is ever entered — a document must be valid text before the
//! runtime ever touches it; a panic
//! anywhere in the run loop is caught here, after the terminal has already
//! been restored by `term::Guard`'s `Drop` running during unwind.
//!
//! [`bootstrap`] does everything up to (but not including) the interactive
//! run loop, so it can be tested against `Mem`: it returns
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

use rune_tui::app::App;
use rune_tui::workspace;
use rune_vfs::{Disk, Vfs};

use cli::CliAction;

mod cli;
mod db_bootstrap;
mod loader;
mod open;

/// `sysexits.h`-flavored exit codes: `EX_USAGE` (a malformed command line —
/// an unrecognised flag, a missing `-w` value, or `-w` pointing somewhere
/// that isn't a directory), `EX_DATAERR` (the file's bytes are not valid
/// data for this program — invalid UTF-8), `EX_IOERR` (the file exists but
/// couldn't be read, or the current directory itself couldn't be read),
/// `EX_SOFTWARE` (an internal error — a recovered panic or a runtime I/O
/// failure).
pub(crate) mod exit_code {
    pub(crate) const USAGE: u8 = 64;
    pub(crate) const DATA_ERR: u8 = 65;
    pub(crate) const IO_ERR: u8 = 74;
    pub(crate) const SOFTWARE: u8 = 70;
}

fn main() -> ExitCode {
    // Read exactly once: every other spot that
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
    // filesystem access in this app — goes through the injected `Vfs`,
    // not a direct `std::fs` call: see `load_sighting`.
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Disk);
    let home = env::var_os("HOME").map(PathBuf::from);

    launch(&vfs, env::args_os().skip(1), &cwd, home.as_deref())
}

/// Everything `main` does after resolving `cwd`/`$HOME` and constructing the
/// real `vfs` — factored out so it's callable with a `Mem` vfs and canned
/// args/cwd/home in tests.
fn launch(
    vfs: &Arc<dyn Vfs + Send + Sync>,
    args: impl Iterator<Item = OsString>,
    cwd: &Path,
    home: Option<&Path>,
) -> ExitCode {
    let mut app = match bootstrap(vfs, args, cwd, home) {
        Ok(app) => app,
        Err(code) => return code,
    };

    let result = panic::catch_unwind(AssertUnwindSafe(|| rune_tui::runtime::run(&mut app)));

    // `app` (an `AppGuard`) drains the writer FIFO and runs clean-shutdown
    // housekeeping (`wal_checkpoint(TRUNCATE)`/`optimize`) in its
    // `Drop`, which now runs on EVERY exit path this function has — normal
    // return below, an early `?`-style return inside `bootstrap` after the
    // guard exists, AND a panic unwinding through this frame — not only the
    // three bootstrap-failure branches `bootstrap` itself handles before an
    // `AppGuard` exists at all. A panic between
    // the store opening and this `catch_unwind` boundary must still drain
    // the writer thread; it used to be skipped.
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
                panic_message(payload)
            );
            ExitCode::from(exit_code::SOFTWARE)
        }
    }
}

/// Wraps `App` so the recovery store's writer thread is always drained,
/// regardless of how this value stops being live — normal end of scope OR
/// a panic unwinding through it. The sole writer
/// of `App.db = None` from this point on; nothing past [`bootstrap`] calls
/// `Db::shutdown` directly.
pub(crate) struct AppGuard(App);

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
/// `runtime::run`. This split is what makes the
/// wiring testable against `Mem`: a test can call this directly and inspect
/// the returned `App` without ever starting the interactive run loop.
fn bootstrap(
    vfs: &Arc<dyn Vfs + Send + Sync>,
    args: impl Iterator<Item = OsString>,
    cwd: &Path,
    home: Option<&Path>,
) -> Result<AppGuard, ExitCode> {
    let launch = parse_launch(vfs.as_ref(), args, cwd)?;
    let (app, db_bootstrap) = open_launch(vfs, &launch, home)?;

    // From here on, a panic unwinding through this function (or `launch`
    // above it, before `catch_unwind`) still drains the writer thread —
    // see `AppGuard`'s own doc.
    let mut app = AppGuard(app);

    let first_doc_id = wire_root_and_extra_files(&mut app, cwd, home, &launch);
    apply_db_bootstrap(&mut app, db_bootstrap, first_doc_id);

    if let Some((left, _)) = &launch.diff {
        install_diff_left(&mut app, vfs.as_ref(), left)?;
    }

    Ok(app)
}

fn install_diff_left(app: &mut AppGuard, vfs: &dyn Vfs, left: &Path) -> Result<(), ExitCode> {
    let bytes = open::read_diff_left(vfs, left)?;
    let left_name = left.file_name().map_or_else(
        || left.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    match rune_tui::diff_view::install(app, bytes, left_name) {
        Ok(()) => Ok(()),
        Err(rune_tui::diff_view::DiffInstallError::InvalidUtf8) => {
            eprintln!(
                "rune: {} is not valid UTF-8 — refusing to open",
                left.display()
            );
            Err(ExitCode::from(exit_code::DATA_ERR))
        }
    }
}

fn parse_launch(
    vfs: &(dyn Vfs + Send + Sync),
    args: impl Iterator<Item = OsString>,
    cwd: &Path,
) -> Result<cli::Cli, ExitCode> {
    let launch = match cli::parse(args, cwd) {
        Ok(CliAction::Version) => {
            println!("rune {}", env!("CARGO_PKG_VERSION"));
            return Err(ExitCode::SUCCESS);
        }
        Ok(CliAction::Help) => {
            println!("{}", cli::USAGE_TEXT);
            return Err(ExitCode::SUCCESS);
        }
        Ok(CliAction::Run(launch)) => launch,
        Err(e) => return Err(open::usage_error(&e)),
    };

    if let Some(dir) = &launch.work_dir
        && let Err(e) = open::validate_work_dir(vfs, dir)
    {
        return Err(open::usage_error(&e));
    }

    Ok(launch)
}

fn diff_right_path(launch: &cli::Cli) -> Option<&Path> {
    launch
        .diff
        .as_ref()
        .map(|(_, right)| right.as_path())
        .or_else(|| launch.files.first().map(PathBuf::as_path))
}

fn open_launch(
    vfs: &Arc<dyn Vfs + Send + Sync>,
    launch: &cli::Cli,
    home: Option<&Path>,
) -> Result<(App, db_bootstrap::DbBootstrap), ExitCode> {
    let Some(path) = diff_right_path(launch) else {
        return Ok(open::open_untitled(vfs, home));
    };
    let path = match workspace::resolve(vfs.as_ref(), path) {
        Ok(path) => path,
        Err(e) => {
            eprintln!("rune: cannot resolve {}: {e}", path.display());
            return Err(ExitCode::from(exit_code::IO_ERR));
        }
    };
    open::open_first_positional(vfs, path, home)
}

fn wire_root_and_extra_files(
    app: &mut AppGuard,
    cwd: &Path,
    home: Option<&Path>,
    launch: &cli::Cli,
) -> rune_tui::document::DocumentId {
    let root = open::resolve_root(
        app.vfs.as_ref(),
        cwd,
        home,
        launch.work_dir.as_deref(),
        diff_right_path(launch),
    );
    app.set_root(root);

    let first_doc_id = app.active;
    open::open_extra_files(app, &launch.files, first_doc_id);
    first_doc_id
}

fn apply_db_bootstrap(
    app: &mut AppGuard,
    db_bootstrap: db_bootstrap::DbBootstrap,
    first_doc_id: rune_tui::document::DocumentId,
) {
    if let Some(banner) = db_bootstrap.banner.clone() {
        rune_tui::messages::error(app, banner);
    }
    app.db_banner = db_bootstrap.banner;
    if let Some(sync_kind) = db_bootstrap.sync_kind {
        app.active_doc_mut().last_sync = Some(sync_kind);
    }
    if let Some(nlink) = db_bootstrap.nlink {
        app.active_doc_mut().nlink = Some(nlink);
        rune_tui::db_ack::warn_hard_links(app, nlink);
    }
    // Hydrated BEFORE binding — the bind chokepoint compares the buffer
    // against what the row reconstructs to, so the adoption must already
    // be in the buffer when the mapping and any re-base are derived.
    if let Some(recovered) = &db_bootstrap.recovered_content {
        let disk_content = app.active_doc_mut().buffer.content().to_string();
        if let rune_tui::document::Hydration::Refused(reason) =
            app.active_doc_mut().hydrate(&disk_content, recovered)
        {
            rune_tui::messages::error(app, format!("crash recovery: {reason}"));
        }
    }
    if let Some(doc_db) = db_bootstrap.doc_db {
        let db_id = doc_db.db_id;
        let row_content = match &db_bootstrap.recovered_content {
            Some(recovered) => recovered.clone(),
            None if doc_db.publish_mode.is_create_only() => String::new(),
            None => app.active_doc_mut().buffer.content().to_string(),
        };
        app.install_or_join_file_binding(db_id, db_bootstrap.expect_obs);
        rune_tui::db_ack::bind_loaded_doc(app, first_doc_id, doc_db, &row_content);
    }
}

/// Extracts a human-readable message from a `catch_unwind` payload. `panic!`
/// with a string literal or `format!` produces `&'static str` or `String`
/// respectively — the two shapes the standard panic machinery actually
/// produces; anything else (a custom payload from a dependency) falls back
/// to a fixed placeholder rather than guessing.
fn panic_message(payload: Box<dyn Any + Send>) -> String {
    let payload = match payload.downcast::<&str>() {
        Ok(s) => return (*s).to_string(),
        Err(payload) => payload,
    };
    payload
        .downcast::<String>()
        .map_or_else(|_| "non-string panic payload".to_string(), |s| *s)
}

fn to_abs_path(input: &str, cwd: &Path) -> PathBuf {
    let path = PathBuf::from(input);
    if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    }
}

#[cfg(test)]
#[path = "bootstrap_tests/mod.rs"]
mod tests;
