//! The launch-action dispatch split out of `main` (plan WP7.S4/WP4.S6): `-w`
//! workspace-root validation and resolution, and the multi-file open loop
//! that opens every positional past the first as its own tab (WP7.S6).

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
use crate::loader::{LoadError, load_sighting};
use crate::{AppGuard, exit_code};

/// `-w`'s own validation (WP7.S4): `dir` (already absolutized by
/// `cli::parse`) must `stat` successfully as a directory. Distinguishes
/// WHY it didn't (plan WP4.S6/[rune-cli 9]) — a nonexistent path, an
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

/// Resolves and opens the first positional, building the `(App,
/// DbBootstrap)` pair `bootstrap` wires up next (plan WP4.S8, split out of
/// `main` to keep it under the 500-line budget). An image path
/// (`kind_for` via `rune_tui::document_support::is_image_path`) never reaches
/// `load_sighting` at all — image bytes are never valid UTF-8 in general, and
/// even a coincidentally UTF-8-clean image must still open read-only, not
/// as editable text — so it's routed through the SAME dispatch every extra
/// positional (and the Explorer) already uses, `workspace::open_path`, via
/// a freshly built untitled `App` as an anchor (there is no buffer to
/// pre-load for an image). That anchor's blank draft is closed once the
/// image opens — it was never edited, so discarding it loses nothing, and
/// a single-file image launch should show exactly the image, not the image
/// plus an empty extra tab. Every other path keeps the pre-WP4 text-load
/// shape unchanged.
pub(crate) fn open_first_positional(
    vfs: &Arc<dyn Vfs + Send + Sync>,
    path: PathBuf,
    home: Option<&Path>,
) -> Result<(App, DbBootstrap), std::process::ExitCode> {
    if rune_tui::document_support::is_image_path(&path) {
        // Issue #78: an image-first launch still needs the session store —
        // later markdown opens (Explorer, extra positionals) must not
        // silently journal nothing for the whole session. The image
        // document itself stays recovery-free either way (`workspace::
        // open_path` binds no `DocDb` for an image), so this only affects
        // documents opened AFTER this one.
        let mut bootstrap = bootstrap_store_only(Arc::clone(vfs), home);
        let mut app = App::new_untitled(Arc::clone(vfs), bootstrap.db.take());
        let blank = app.active;
        let opened = workspace::open_path(&mut app, &path);
        if let Some(image_id) = opened
            && image_id != blank
        {
            // A scratch sink: no runtime/terminal exists yet at this point
            // in the CLI bootstrap (plan WP4.S8), and the blank draft being
            // closed here is never an image document, so `close_now`'s
            // image-delete branch (WP5.S7) is a no-op on this path either
            // way.
            let _ =
                workspace::close_now(&mut app, blank, &mut rune_tui::runtime::Effects::default());
        }
        return Ok((
            app,
            DbBootstrap {
                banner: bootstrap.banner,
                ..DbBootstrap::default()
            },
        ));
    }

    // One sighting decides both "does this path exist" and, if so, the
    // buffer's bytes (issue #77) — never a separate `vfs.stat` plus a
    // separate read of the same path.
    let sighting = match load_sighting(vfs.as_ref(), &path) {
        Ok(sighting) => sighting,
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
    let buffer = match &sighting {
        // `load_sighting` already refused invalid UTF-8, so this re-decode
        // is a formality, not a second gate — an `Err` here can only mean a
        // producer bug upstream, surfaced the same way any other unreadable
        // file is, rather than assumed away.
        Some(sighting) => match String::from_utf8(sighting.bytes.clone()) {
            Ok(text) => rune_core::buffer::Buffer::new(text),
            Err(e) => {
                eprintln!(
                    "rune: {} is not valid UTF-8 — refusing to open (file left untouched)",
                    path.display()
                );
                let _ = e;
                return Err(std::process::ExitCode::from(exit_code::DATA_ERR));
            }
        },
        None => rune_core::buffer::Buffer::new(""),
    };

    // The recovery store (plan WP5.S2/S4). `rune_db::load` itself requires
    // the target to already exist on disk (`vfs.resolve`+`vfs.read` with no
    // NotFound-tolerant branch, unlike `load_sighting` above), so a missing
    // path has nothing to `Load` — but that is not "no recovery this
    // launch": a named-but-not-yet-created file is exactly a recovery-backed
    // untitled draft that already knows its name, so it binds a scratch row
    // instead, the same route the no-positional launch takes
    // (`bootstrap_new_file`). Any bootstrap failure is still non-fatal
    // (protect the user's words over every other feature) — reported to
    // stderr, not to the TUI (which hasn't started yet), and the editor
    // proceeds with `app.db = None`.
    let mut db_bootstrap = if let Some(sighting) = sighting {
        bootstrap_db(Arc::clone(vfs), &path, home, sighting)
    } else {
        bootstrap_new_file(Arc::clone(vfs), home)
    };

    // The buffer stays exactly what `load_sighting` read off disk here —
    // adopting `recovered_content` goes through the same hydration
    // chokepoint (`Document::hydrate`, plan WP5.S2) `db::handle_load_ack`
    // uses, once `App::new` exists to hold it. Pre-replacing the buffer
    // here (as this used to) would skip that chokepoint's suspicion
    // check entirely.
    let app = App::new(buffer, Some(path), Arc::clone(vfs), db_bootstrap.db.take());
    Ok((app, db_bootstrap))
}

/// Builds the default no-positional launch: an untitled document genuinely
/// backed by the recovery store (plan WP3, "the untitled draft is really
/// recovery-backed" — the fix for `crates/rune-tui/TODO.md`'s now-resolved
/// "no recovery journal for the default untitled document" entry).
/// `bootstrap_untitled_db` does the actual store/scratch-row work
/// (opening/creating/recovering rows, GC); this only wires its result onto a
/// freshly constructed `App`. `scratch_docs` is ordered newest first: the
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
        adopt_scratch_doc(&mut app, active, first);
    }
    for extra in docs {
        let id = app.open_document(rune_core::buffer::Buffer::new(""));
        let name = workspace::next_untitled_name(&app);
        if let Some(doc) = app.doc_mut(id) {
            doc.display_name = Some(name);
        }
        adopt_scratch_doc(&mut app, id, extra);
    }

    (
        app,
        DbBootstrap {
            banner: bootstrap.banner,
            ..DbBootstrap::default()
        },
    )
}

/// Binds `scratch.db_id` onto `id`'s `Document` and, when there is actually
/// recovered text, adopts it through `Document::hydrate` — the suspicion
/// check, the synthetic bridge `Step` so post-restart undo reaches
/// the recovered text in one step, and a refusal surfaced as a status rather
/// than silently applied (mirrors `bootstrap`'s own handling of `rune_db::
/// load`'s `recovered_content`). `bind_new` is always `true`: a scratch
/// document — recovered or freshly minted — has never been bound to a real
/// file, so its NEXT save must still go through the create-only path.
/// `expect_obs` is `0`, a fabricated `ObsId` that is never actually queried
/// (`materialize::prepare_materialize` skips the CAS-baseline lookup
/// entirely when `bind_new` is set) — never handed to a caller that would
/// treat it as a genuine baseline.
fn adopt_scratch_doc(app: &mut App, id: DocumentId, scratch: ScratchDoc) {
    if let Some(doc) = app.doc_mut(id) {
        doc.db = Some(rune_tui::db::DocDb::new(scratch.db_id, true, 0));
    }
    app.bind_file(scratch.db_id, 0);
    if scratch.content.is_empty() {
        return;
    }
    let Some(doc) = app.doc_mut(id) else { return };
    let disk_content = doc.buffer.content().to_string();
    if let rune_tui::document::Hydration::Refused(reason) =
        doc.hydrate(&disk_content, &scratch.content)
    {
        rune_tui::messages::error(app, format!("crash recovery: {reason}"));
    }
    // Dirty is a content comparison now (plan WP1) — `hydrate` no longer
    // marks it itself, so every hydration site re-derives it explicitly,
    // same as `bootstrap`'s and `db_ack::handle_load_ack`'s own hydration
    // sites.
    app.recompute_dirty(id);
}

/// The first positional is already open (in `bootstrap`) and stays the
/// active, displayed document — every REMAINING file opens as its own tab
/// through the same path the Explorer uses. A failure there posts its own
/// message into the log via `workspace::open_path` itself instead of
/// aborting startup — a log has no single-slot limit, so a later failure
/// never silently overwrites an earlier one; every failure across the
/// whole batch gets its own entry.
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
    use rune_vfs::Mem;

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
            ScratchDoc {
                db_id: 1,
                content: "recovered draft text".to_string(),
            },
        );

        assert!(
            app.doc(id).expect("doc exists").is_dirty(),
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
