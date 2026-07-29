//! `rune`: the CLI entry point (plan Context, WP5.S2; strict argument
//! parsing and multi-file launch added WP7). `cli::parse` (abs-path'ing
//! every positional and `-w`'s value) hands back `--version`/`--help`, a
//! parsed [`cli::Cli`], or a rejected command line; the first positional
//! opens through the load path below exactly as a single-file launch
//! always has, every remaining one opens as its own tab, and the first
//! stays the active document. No file opens an empty untitled document; a
//! nonexistent path opens an empty buffer (created on first save via
//! `RENAME_EXCL`); invalid UTF-8 is refused at load, before the TUI is
//! ever entered (CONSTITUTION §0, plan decision 4); a panic anywhere in
//! the run loop is caught here, after the terminal has already been
//! restored by `term::Guard`'s `Drop` running during unwind.

use std::any::Any;
use std::env;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use rune_core::buffer::{Buffer, BufferError};
use rune_db::{DbEvent, OpOutcome, Store};
use rune_tui::app::App;
use rune_tui::db::{Db, DbBridge, DocDb};
use rune_tui::{workspace, workspaceroot};
use rune_vfs::{Disk, FileKind, Vfs};

use cli::{CliAction, CliError};

mod cli;

/// `sysexits.h`-flavored exit codes: `EX_USAGE` (a malformed command line —
/// an unrecognised flag, a missing `-w` value, or `-w` pointing somewhere
/// that isn't a directory), `EX_DATAERR` (the file's bytes are not valid
/// data for this program — invalid UTF-8), `EX_IOERR` (the file exists but
/// couldn't be read), `EX_SOFTWARE` (an internal error — a recovered panic
/// or a runtime I/O failure).
mod exit_code {
    pub const USAGE: u8 = 64;
    pub const DATA_ERR: u8 = 65;
    pub const IO_ERR: u8 = 74;
    pub const SOFTWARE: u8 = 70;
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();

    let launch = match cli::parse(args.into_iter()) {
        Ok(CliAction::Version) => {
            println!("rune {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Ok(CliAction::Help) => {
            println!("{}", cli::USAGE_TEXT);
            return ExitCode::SUCCESS;
        }
        Ok(CliAction::Run(launch)) => launch,
        Err(e) => return usage_error(&e),
    };

    // Constructed before the load so the whole load path — like every other
    // filesystem access in this app (CONSTITUTION §1.4.9) — goes through the
    // injected `Vfs`, not a direct `std::fs` call: see `load_buffer`.
    let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Disk);

    // `-w`'s existence/directory-ness check is the documented §1.4.9
    // launch-bootstrap exception, same class as `workspaceroot::resolve`'s
    // own `read_dir` walk below (WP7.S4).
    if let Some(dir) = &launch.work_dir
        && let Err(e) = validate_work_dir(vfs.as_ref(), dir)
    {
        return usage_error(&e);
    }

    let (mut app, db_bootstrap) = if let Some(path) = launch.files.first() {
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
        // None`. This launch mode is otherwise silent about running with zero
        // crash protection (plan [rune-cli 3]) — every OTHER way this session
        // can end up without a recovery journal (a degraded open ladder, a
        // failed `Load`) already sets `app.db_banner`, so this one does too,
        // rather than leaving the user with no indication at all.
        let mut db_bootstrap = if file_existed {
            bootstrap_db(Arc::clone(&vfs), &path)
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
        let app = App::new(buffer, Some(path), vfs, db_bootstrap.db.take());
        (app, db_bootstrap)
    } else {
        // No positional files — open the default untitled document
        // (`App::new_untitled`). The Go implementation uses
        // `nextUntitledName` to pick the first "Untitled N" not already
        // used by open tabs; at startup there are none, so this is always
        // "Untitled 1". No file on disk means no recovery store to
        // hydrate either — see `App::new_untitled`'s own docs.
        (App::new_untitled(vfs), DbBootstrap::default())
    };

    // `-w` wins outright; otherwise walk up from `cwd` (falling back to the
    // first file's parent) for a `.git`/`.obsidian` marker. Reading `cwd`
    // and `$HOME` here is the same documented §1.4.9 launch-bootstrap
    // exception as the `-w` validation above (WP7.S5) — every actual
    // directory read during the walk itself goes through `app.vfs`.
    let root = match &launch.work_dir {
        Some(dir) => dir.clone(),
        None => {
            let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            let home = env::var_os("HOME").map(PathBuf::from);
            workspaceroot::resolve(
                app.vfs.as_ref(),
                &cwd,
                home.as_deref(),
                launch.files.first().map(|p| p.as_path()),
            )
        }
    };
    app.set_root(root);

    // The first positional is already open (above) and stays the active,
    // displayed document (Go treats it as the awaited display document and
    // the rest as tabs) — every REMAINING file opens as its own tab through
    // the same path the Explorer uses, which reports its own failures via
    // the banner instead of aborting startup (WP7.S6).
    let first_doc_id = app.active;
    for extra in launch.files.iter().skip(1) {
        workspace::open_path(&mut app, extra);
    }
    if launch.files.len() > 1 {
        workspace::switch_to(&mut app, first_doc_id);
    }

    app.db_banner = db_bootstrap.banner;
    // Install the real hardware probe (plan WP5.S8) — production is the
    // ONLY place `HidSpaceProbe` is installed; every test and the fuzzer
    // keep `App::new`'s inert `NullProbe` default.
    app.space_probe = Box::new(rune_tui::keystate::HidSpaceProbe);
    // Prime `leader_available`'s cache now, at startup, rather than letting
    // the first `space+x`/`e`/`t` press pay for it: `leader_available` is a
    // `OnceLock` (plan WP3.S3), so this one-shot check runs here instead of
    // on the user's first keystroke.
    let _ = rune_tui::keystate::leader_available();
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

/// Opens the recovery store at `versioning::production_db_path()` and
/// hydrates `path` through it (plan WP5.S2/S4), BEFORE the TUI ever starts
/// (`runtime::run` hasn't been called yet — no `Sender<Msg>` exists; see
/// `db::DbBridge`'s doc comment for why hydration blocks on the bridge's
/// OWN buffer instead). Never fatal to the editor: any failure here is
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

/// `-w`'s own validation (WP7.S4): `dir` (already absolutized by
/// `cli::parse`) must `stat` successfully as a directory. Split out from
/// `main` so it's exercisable against `Mem` in tests, exactly like
/// `load_buffer` above.
fn validate_work_dir(vfs: &dyn Vfs, dir: &Path) -> Result<(), CliError> {
    match vfs.stat(dir) {
        Ok(stat) if stat.kind == FileKind::Dir => Ok(()),
        _ => Err(CliError::NotADirectory(dir.to_path_buf())),
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
}
