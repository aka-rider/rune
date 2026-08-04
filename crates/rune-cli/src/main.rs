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
        Err(e) => return Err(open::usage_error(&e)),
    };

    // `-w`'s existence/directory-ness check is the documented §1.4.9
    // launch-bootstrap exception, same class as `workspaceroot::resolve`'s
    // own `read_dir` walk below (WP7.S4).
    if let Some(dir) = &launch.work_dir
        && let Err(e) = open::validate_work_dir(vfs.as_ref(), dir)
    {
        return Err(open::usage_error(&e));
    }

    let (app, db_bootstrap) = if let Some(path) = launch.files.first() {
        // The SAME resolution chokepoint every other open path (every
        // extra positional below, and the Explorer) already funnels
        // through — `workspace::resolve` — so the first positional can
        // never bind as an unresolved spelling that a later open of the
        // identical underlying file (a symlink, a `..` segment, a
        // duplicated absolute path) would fail to recognize as the same
        // document (plan [rune-cli 2]). `open::open_first_positional`
        // (plan WP4.S8) is what actually decides whether that resolved
        // path opens as text or as a read-only image document.
        let path = workspace::resolve(vfs.as_ref(), path);
        open::open_first_positional(&vfs, path, home.as_deref())?
    } else {
        // No positional files — open the default untitled document
        // (`App::new_untitled`), genuinely recovery-backed (plan WP3):
        // `open::open_untitled` opens/recovers its own scratch row through
        // the SAME recovery store a file launch uses, rather than always
        // starting with `db: None`.
        open::open_untitled(&vfs, home.as_deref())
    };

    // From here on, a panic unwinding through this function (or `launch`
    // above it, before `catch_unwind`) still drains the writer thread —
    // see `AppGuard`'s own doc.
    let mut app = AppGuard(app);

    // `-w` wins outright; otherwise walk up from `cwd` (falling back to the
    // first file's parent) for a `.git`/`.obsidian` marker. Every actual
    // directory read during the walk goes through `app.vfs`.
    let root = open::resolve_root(
        app.vfs.as_ref(),
        &cwd,
        home.as_deref(),
        launch.work_dir.as_deref(),
        launch.files.first().map(|p| p.as_path()),
    );
    app.set_root(root);

    let first_doc_id = app.active;
    open::open_extra_files(&mut app, &launch.files, first_doc_id);

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
        // Dirty is a content comparison now (plan WP1) — `hydrate` no
        // longer marks it itself, so every hydration site re-derives it
        // explicitly (CONSTITUTION §1.4.8).
        app.recompute_dirty(first_doc_id);
    }

    Ok(app)
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

fn to_abs_path(input: &str, cwd: &Path) -> PathBuf {
    let path = PathBuf::from(input);
    if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    }
}

#[cfg(test)]
#[path = "bootstrap_tests.rs"]
mod tests;
