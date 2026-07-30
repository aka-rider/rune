//! The launch-action dispatch split out of `main` (plan WP7.S4/WP4.S6): `-w`
//! workspace-root validation and resolution, and the multi-file open loop
//! that opens every positional past the first as its own tab (WP7.S6).

use std::path::{Path, PathBuf};

use rune_tui::app::App;
use rune_tui::banner;
use rune_tui::document::DocumentId;
use rune_tui::{workspace, workspaceroot};
use rune_vfs::{FileKind, Vfs};

use crate::cli::CliError;
use crate::{AppGuard, exit_code};

/// `-w`'s own validation (WP7.S4): `dir` (already absolutized by
/// `cli::parse`) must `stat` successfully as a directory. Distinguishes
/// WHY it didn't (plan WP4.S6/[rune-cli 9]) — a nonexistent path, an
/// existing non-directory, or some other `stat` failure (permission
/// denied, an I/O error) each get their own [`CliError`] instead of one
/// wildcard "not a directory" collapsing all three. Split out from
/// `bootstrap` so it's exercisable against `Mem` in tests, exactly like
/// `load_buffer` in the parent module.
pub(crate) fn validate_work_dir(vfs: &dyn Vfs, dir: &Path) -> Result<(), CliError> {
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
/// [`crate::cli::USAGE_TEXT`], both to stderr (WP7.S3).
pub(crate) fn usage_error(e: &CliError) -> std::process::ExitCode {
    eprintln!("rune: {e}");
    eprintln!("{}", crate::cli::USAGE_TEXT);
    std::process::ExitCode::from(exit_code::USAGE)
}

/// `-w` wins outright; otherwise walk up from `cwd` (falling back to the
/// first file's parent) for a `.git`/`.obsidian` marker. Every actual
/// directory read during the walk goes through the injected `vfs`.
pub(crate) fn resolve_root(
    vfs: &dyn Vfs,
    cwd: &Path,
    home: Option<&Path>,
    work_dir: Option<&Path>,
    first_file: Option<&Path>,
) -> PathBuf {
    match work_dir {
        Some(dir) => dir.to_path_buf(),
        None => workspaceroot::resolve(vfs, cwd, home, first_file),
    }
}

/// The first positional is already open (in `bootstrap`) and stays the
/// active, displayed document (Go treats it as the awaited display document
/// and the rest as tabs) — every REMAINING file opens as its own tab
/// through the same path the Explorer uses (WP7.S6). A failure there
/// reports into the error banner instead of aborting startup; every
/// failure across the whole batch is accumulated and reported ONCE (plan
/// WP4.S6/[rune-cli 7]) rather than letting only the last one survive a
/// string of "the modal replaces on ties" overwrites.
pub(crate) fn open_extra_files(app: &mut AppGuard, files: &[PathBuf], first_doc_id: DocumentId) {
    let mut open_errors: Vec<String> = Vec::new();
    for extra in files.iter().skip(1) {
        if workspace::open_path(app, extra).is_none()
            && let Some(text) = take_error_banner(app)
        {
            open_errors.push(text);
        }
    }
    if files.len() > 1 {
        workspace::switch_to(app, first_doc_id);
    }
    if !open_errors.is_empty() {
        banner::report_error(app, combine_open_errors(&open_errors));
    }
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use rune_vfs::Mem;

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
}
