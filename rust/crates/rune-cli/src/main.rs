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

use rune_core::buffer::{Buffer, BufferError};
use rune_core::vfs::{Disk, Vfs};
use rune_tui::app::App;

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

    let mut app = App::new(buffer, Some(path), vfs);

    let result = panic::catch_unwind(AssertUnwindSafe(|| rune_tui::runtime::run(&mut app)));
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
    use rune_core::vfs::Mem;

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
