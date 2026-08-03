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
use db_bootstrap::DbBootstrap;

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
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use rune_vfs::Mem;

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

    /// Plan WP4.S8: a `.png` first positional bootstraps through the SAME
    /// `workspace::open_path` dispatch every extra positional uses (built
    /// via the untitled `App` constructor as an anchor), rather than
    /// `load_buffer`'s text-only path — which would reject the PNG's bytes
    /// outright as invalid UTF-8, exactly the failure this restructuring
    /// exists to route around. Exactly one document is left open (the
    /// blank untitled anchor is closed once the image opens), it is the
    /// active one, and it is read-only.
    #[test]
    fn launch_first_positional_png_bootstraps_as_a_read_only_image_document() {
        let vfs = Mem::new();
        vfs.save_atomic(
            Path::new("/vault/x.png"),
            &[0x89, b'P', b'N', b'G', 0, 0, 0, 0],
        )
        .expect("seed a (fake) png");

        let app = bootstrap(
            Arc::new(vfs),
            vec![OsString::from("/vault/x.png")].into_iter(),
            PathBuf::from("/"),
            None,
        )
        .expect("bootstrap should succeed for an image first positional");

        assert_eq!(
            app.documents.len(),
            1,
            "the blank untitled anchor must be closed once the image opens"
        );
        assert!(app.doc(app.active).is_some_and(|d| d.read_only));
        assert!(
            app.doc(app.active)
                .is_some_and(|d| d.file_path.as_deref() == Some(Path::new("/vault/x.png")))
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
