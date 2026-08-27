//! `load_dir_cmd`/`read_file_cmd`/`load_search_history_cmd` — split out of
//! `runtime/mod.rs` for the 500-line budget, the same reason
//! `highlight_cmd`/`timer`/`filesearch_cmd` were.

use std::path::PathBuf;
use std::sync::Arc;

use rune_vfs::Vfs;

use super::{Cmd, CmdError, DirCause, Msg, RecentsKind, RecentsResult};

/// Reads `root`'s children off-thread via `vfs.read_dir` and
/// replies with `Msg::DirLoaded`, or `Msg::Error` on a read failure — the
/// Explorer's own boundary Msg, called from `explorer_keys::handle_key` (Open on
/// a directory, Backspace to the parent) and from `pane::handle_global_
/// command`'s `FocusExplorer` arm (the very first load). The
/// filesystem is reached only through the injected `Vfs`; this I/O
/// never runs inline in `update`, only inside a spawned `Cmd`. `generation`
/// is echoed back verbatim on the `Msg::DirLoaded` reply — every call site
/// passes `Explorer::request_generation` AFTER bumping it, so a later
/// request's reply can never be shadowed by an earlier, slower one landing
/// after it (review fix).
pub fn load_dir_cmd(
    vfs: Arc<dyn Vfs + Send + Sync>,
    root: PathBuf,
    cause: DirCause,
    generation: crate::generation::DirLoadGen,
) -> Cmd {
    Cmd::read_dir(move || match vfs.read_dir(&root) {
        Ok(entries) => Some(Msg::DirLoaded {
            root,
            entries,
            cause,
            generation,
        }),
        Err(e) => Some(Msg::Posted {
            severity: crate::messages::Severity::Warn,
            text: format!("could not list {}: {e}", root.display()),
        }),
    })
}

/// Reads `path` off-thread via `rune_vfs::get` —
/// `workspace::open_path_async`'s only `Cmd`, and `load_dir_cmd`'s single-
/// file counterpart. `anchor` is opaque data here, just carried through to
/// the `Msg::FileOpened` reply unchanged — this `Cmd` never resolves it
/// itself (that needs the target's own catalogue, which doesn't exist
/// until the document is open).
pub fn read_file_cmd(
    vfs: Arc<dyn Vfs + Send + Sync>,
    path: PathBuf,
    anchor: Option<rune_nav::Anchor>,
) -> Cmd {
    Cmd::read_file(move || {
        let result = rune_vfs::get(vfs.as_ref(), &path, Some(rune_vfs::MAX_DOCUMENT_BYTES))
            .map(|sighting| sighting.bytes)
            .map_err(CmdError::from);
        Some(Msg::FileOpened {
            path,
            result,
            anchor,
            preview_generation: None,
        })
    })
}

/// Loads the search bar's MRU history off-thread through a cloned
/// `ReaderQuery` — the reader thread's own connection, never
/// the writer's, so this can never contend with or block on an in-flight
/// recovery write. Always replies with `Msg::RecentsLoaded { kind:
/// RecentsKind::Search, .. }`, `generation` carried through unchanged: a
/// query failure becomes `result: Err(..)` rather than `Msg::Error`/
/// `Msg::Warning` directly, so `search::handle_history_loaded` can apply
/// the same stale-generation check to a failure as to a success instead of
/// always surfacing a message even for a reply nobody's still waiting on.
pub fn load_search_history_cmd(
    reader: rune_db::ReaderQuery,
    generation: crate::generation::SearchHistoryGen,
) -> Cmd {
    Cmd::search_history(move || {
        let result = reader
            .query(rune_db::ReaderRequestKind::RecentSearches { limit: 200 })
            .map(|reply| match reply {
                rune_db::ReaderReply::RecentSearches(entries) => entries,
                _ => Vec::new(),
            })
            .map_err(CmdError::from);
        Some(Msg::RecentsLoaded {
            kind: RecentsKind::Search,
            generation: generation.raw(),
            result: RecentsResult::Strings(result),
        })
    })
}
