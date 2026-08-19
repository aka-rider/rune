//! The launch-action dispatch split out of `main`: `-w` workspace-root
//! validation and resolution, and the multi-file open loop that opens
//! every positional past the first as its own tab.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rune_tui::app::App;
use rune_tui::document::DocumentId;
use rune_tui::{workspace, workspaceroot};
use rune_vfs::{FileKind, Vfs};

use crate::cli::CliError;
use crate::db_bootstrap::{
    DbBootstrap, ScratchDoc, bootstrap_db, bootstrap_new_file, bootstrap_store_only,
    bootstrap_untitled_db,
};
use crate::loader::{LoadError, LoadedFile, load_sighting};
use crate::{AppGuard, exit_code};

/// `-w`'s own validation: `dir` (already absolutized by
/// `cli::parse`) must `stat` successfully as a directory. Distinguishes
/// WHY it didn't — a nonexistent path, an
/// existing non-directory, or some other `stat` failure (permission
/// denied, an I/O error) each get their own [`CliError`] instead of one
/// wildcard "not a directory" collapsing all three. Split out from
/// `bootstrap` so it's exercisable against `Mem` in tests, exactly like
/// `load_sighting` in the parent module.
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
/// [`crate::cli::USAGE_TEXT`], both to stderr.
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
    work_dir.map_or_else(
        || workspaceroot::resolve(vfs, cwd, home, first_file),
        Path::to_path_buf,
    )
}

/// Resolves and opens the first positional, building the `(App,
/// DbBootstrap)` pair `bootstrap` wires up next — split out of
/// `main` to keep it under the 500-line budget. An image path
/// (`kind_for` via `rune_tui::document_support::is_image_path`) never reaches
/// `load_sighting` at all — image bytes are never valid UTF-8 in general, and
/// even a coincidentally UTF-8-clean image must still open read-only, not
/// as editable text — so [`open_first_image`] routes it through the SAME
/// dispatch every extra positional (and the Explorer) already uses,
/// `workspace::open_path`. Every other path keeps the ordinary text-load
/// shape unchanged, in [`open_first_text`].
pub(crate) fn open_first_positional(
    vfs: &Arc<dyn Vfs + Send + Sync>,
    path: PathBuf,
    home: Option<&Path>,
) -> Result<(App, DbBootstrap), std::process::ExitCode> {
    if rune_tui::document_support::is_image_path(&path) {
        Ok(open_first_image(vfs, &path, home))
    } else {
        open_first_text(vfs, path, home)
    }
}

/// An image-first launch still needs the session store —
/// later markdown opens (Explorer, extra positionals) must not
/// silently journal nothing for the whole session. The image
/// document itself stays recovery-free either way (`workspace::
/// open_path` binds no `DocDb` for an image), so this only affects
/// documents opened AFTER this one. Opens through a freshly built
/// untitled `App` as an anchor (there is no buffer to pre-load for an
/// image); that anchor's blank draft is closed once the image opens — it
/// was never edited, so discarding it loses nothing, and a single-file
/// image launch should show exactly the image, not the image plus an
/// empty extra tab.
fn open_first_image(
    vfs: &Arc<dyn Vfs + Send + Sync>,
    path: &Path,
    home: Option<&Path>,
) -> (App, DbBootstrap) {
    let mut bootstrap = bootstrap_store_only(Arc::clone(vfs), home);
    let mut app = App::new_untitled(Arc::clone(vfs), bootstrap.db.take());
    let blank = app.active;
    let opened = workspace::open_path(&mut app, path);
    if let Some(image_id) = opened
        && image_id != blank
    {
        // A scratch sink: no runtime/terminal exists yet at this point
        // in the CLI bootstrap, and the blank draft being
        // closed here is never an image document, so `close_now`'s
        // image-delete branch is a no-op on this path either
        // way.
        match workspace::close_now(&mut app, blank, &mut rune_tui::runtime::Effects::default()) {
            workspace::CloseOutcome::Closed => {}
            workspace::CloseOutcome::Unknown => {
                eprintln!("rune: internal error: failed to close the placeholder draft");
            }
        }
    }
    (
        app,
        DbBootstrap {
            banner: bootstrap.banner,
            ..DbBootstrap::default()
        },
    )
}

fn open_first_text(
    vfs: &Arc<dyn Vfs + Send + Sync>,
    path: PathBuf,
    home: Option<&Path>,
) -> Result<(App, DbBootstrap), std::process::ExitCode> {
    // One sighting decides both "does this path exist" and, if so, the
    // buffer's text — never a separate `vfs.stat` plus a separate read of
    // the same path, and `load_sighting` hands back text already validated
    // as UTF-8, so there is no second decode/validation branch here.
    let loaded = match load_sighting(vfs.as_ref(), &path) {
        Ok(loaded) => loaded,
        Err(LoadError::InvalidUtf8) => {
            eprintln!(
                "rune: {} is not valid UTF-8 — refusing to open (file left untouched)",
                path.display()
            );
            return Err(std::process::ExitCode::from(exit_code::DATA_ERR));
        }
        Err(LoadError::Io(e)) => {
            eprintln!("rune: failed to read {}: {e}", path.display());
            return Err(std::process::ExitCode::from(exit_code::IO_ERR));
        }
    };

    // The recovery store. `rune_db::load` itself requires the target to
    // already exist on disk (`vfs.resolve`+`vfs.read` with no
    // NotFound-tolerant branch, unlike `load_sighting` above), so a missing
    // path has nothing to `Load` — but that is not "no recovery this
    // launch": a named-but-not-yet-created file is exactly a recovery-backed
    // untitled draft that already knows its name, so it binds a scratch row
    // instead, the same route the no-positional launch takes
    // (`bootstrap_new_file`). Any bootstrap failure is still non-fatal
    // (protect the user's words over every other feature) — reported to
    // stderr, not to the TUI (which hasn't started yet), and the editor
    // proceeds with `app.db = None`.
    //
    // The buffer stays exactly what `load_sighting` read off disk here —
    // adopting `recovered_content` goes through the same hydration
    // chokepoint (`Document::hydrate`) `db::handle_load_ack` uses, once
    // `App::new` exists to hold it. Pre-replacing the buffer here would skip
    // that chokepoint's suspicion check entirely.
    let (buffer, mut db_bootstrap) = match loaded {
        Some(LoadedFile { sighting, text }) => (
            rune_core::buffer::Buffer::new(text),
            bootstrap_db(Arc::clone(vfs), &path, home, sighting),
        ),
        None => (
            rune_core::buffer::Buffer::new(""),
            bootstrap_new_file(Arc::clone(vfs), &path, home),
        ),
    };

    let app = App::new(buffer, Some(path), Arc::clone(vfs), db_bootstrap.db.take());
    Ok((app, db_bootstrap))
}

/// Builds the default no-positional launch: an untitled document genuinely
/// backed by the recovery store. `bootstrap_untitled_db` does the actual
/// store/scratch-row work (opening/creating/recovering rows, GC); this only
/// wires its result onto a freshly constructed `App`. `scratch_docs` is
/// ordered newest first: the
/// first entry adopts the already-open default document (through
/// `Document::hydrate`, the same chokepoint every other hydration route
/// uses) and becomes the active tab; every remaining recovered draft opens
/// as its OWN background tab, bound to its OWN row (never a fresh row
/// copying the text in — `ScratchDoc`'s own doc comment explains why).
pub(crate) fn open_untitled(
    vfs: &Arc<dyn Vfs + Send + Sync>,
    home: Option<&Path>,
) -> (App, DbBootstrap) {
    let mut bootstrap = bootstrap_untitled_db(Arc::clone(vfs), home);

    let mut app = App::new_untitled(Arc::clone(vfs), bootstrap.db.take());

    let mut docs = bootstrap.scratch_docs.into_iter();
    if let Some(first) = docs.next() {
        let active = app.active;
        adopt_scratch_doc(&mut app, active, &first);
    }
    for extra in docs {
        let id = app.open_document(rune_core::buffer::Buffer::new(""));
        let name = workspace::next_untitled_name(&app);
        if let Some(doc) = app.doc_mut(id) {
            doc.display_name = Some(name);
        }
        adopt_scratch_doc(&mut app, id, &extra);
    }

    (
        app,
        DbBootstrap {
            banner: bootstrap.banner,
            ..DbBootstrap::default()
        },
    )
}

/// Binds `scratch.db_id` onto `id`'s `Document` and adopts any recovered
/// text — delegated to `rune_tui::db_ack::adopt_scratch_doc`, the same
/// chokepoint every store bind funnels through, so the launch-time path
/// can never drift from the async-ack one.
fn adopt_scratch_doc(app: &mut App, id: DocumentId, scratch: &ScratchDoc) {
    rune_tui::db_ack::adopt_scratch_doc(app, id, scratch.db_id, &scratch.content);
}

/// The first positional is already open (in `bootstrap`) and stays the
/// active, displayed document — every REMAINING file opens as its own tab
/// through the same path the Explorer uses. A failure there posts its own
/// message into the log via `workspace::open_path` itself instead of
/// aborting startup — a log has no single-slot limit, so a later failure
/// never silently overwrites an earlier one; every failure across the
/// whole batch gets its own entry.
pub(crate) fn read_diff_left(
    vfs: &dyn Vfs,
    path: &Path,
) -> Result<Vec<u8>, std::process::ExitCode> {
    match vfs.read(path) {
        Ok(bytes) => Ok(bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("rune: {} not found", path.display());
            Err(std::process::ExitCode::from(exit_code::IO_ERR))
        }
        Err(e) => {
            eprintln!("rune: failed to read {}: {e}", path.display());
            Err(std::process::ExitCode::from(exit_code::IO_ERR))
        }
    }
}

pub(crate) fn open_extra_files(app: &mut AppGuard, files: &[PathBuf], first_doc_id: DocumentId) {
    for extra in files.iter().skip(1) {
        workspace::open_path(app, extra);
    }
    if files.len() > 1 {
        workspace::switch_to(app, first_doc_id);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use rune_vfs::{Mem, VfsTestExt};

    /// Finding 1 regression: `adopt_scratch_doc` hydrates a recovered
    /// scratch draft into an otherwise-empty buffer but must also re-derive
    /// dirty — dirtiness no longer falls out of `Document::hydrate` itself
    /// since `mark_dirty_from_hydration` was deleted, so every hydration
    /// site (this one included) must call
    /// `App::recompute_dirty` explicitly or the recovered text renders
    /// clean while `saved_content` is still empty.
    #[test]
    fn adopt_scratch_doc_marks_the_document_dirty() {
        let vfs: Arc<dyn Vfs + Send + Sync> = Arc::new(Mem::new());
        let mut app = App::new_untitled(Arc::clone(&vfs), None);
        let id = app.active;

        adopt_scratch_doc(
            &mut app,
            id,
            &ScratchDoc {
                db_id: 1,
                content: "recovered draft text".to_string(),
            },
        );

        assert!(
            app.doc(id).expect("doc exists").dirty_for_render(),
            "a recovered draft that differs from its (empty) baseline must be dirty"
        );
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
}
