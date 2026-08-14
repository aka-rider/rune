//! The fuzzy file finder's recents-load `Cmd` constructor — split out of
//! `runtime.rs` itself (500-line budget), the same reason `highlight_cmd`/
//! `snapshot_timer` were.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rune_vfs::Vfs;

use crate::filesearch::Candidate;

use super::{Cmd, Msg};

/// Loads the finder's MRU document list off-thread through a cloned
/// `ReaderQuery` — the reader thread's own connection, never the writer's,
/// so this can never contend with or block on an in-flight recovery write.
/// Reuses `CmdKind::SearchHistory`: the same off-thread reader-connection
/// resource `load_search_history_cmd` already names, and the same
/// fuzz-driver exemption that comes with it (the session fuzzer's step
/// executor classifies neither kind as reachable, so a finder-recents reply
/// stays unexercised there exactly like search history's own). Always
/// replies with `Msg::FileSearchRecentsLoaded`, `generation` carried
/// through unchanged — an unexpected reader reply variant folds into an
/// `Err` on the SAME `Msg` rather than a silently empty list, so a
/// mis-wiring surfaces in the message pane instead of an eternally empty
/// finder.
pub fn load_filesearch_recents_cmd(
    reader: rune_db::ReaderQuery,
    vfs: Arc<dyn Vfs + Send + Sync>,
    root: PathBuf,
    generation: crate::generation::Generation,
) -> Cmd {
    Cmd::search_history(move || {
        let result = load(&reader, vfs.as_ref(), &root);
        Some(Msg::FileSearchRecentsLoaded { generation, result })
    })
}

fn load(
    reader: &rune_db::ReaderQuery,
    vfs: &dyn Vfs,
    root: &Path,
) -> Result<Vec<Candidate>, String> {
    let reply = reader
        .query(rune_db::ReaderRequestKind::RecentDocuments { limit: 100 })
        .map_err(|e| e.to_string())?;
    let paths = match reply {
        rune_db::ReaderReply::RecentDocuments(paths) => paths,
        rune_db::ReaderReply::Pong
        | rune_db::ReaderReply::Blob(_)
        | rune_db::ReaderReply::RecentSearches(_) => {
            return Err("unexpected reader reply".to_string());
        }
    };
    // A `documents` row can outlive the file it named (deleted, renamed out
    // from under the store) — `vfs.stat` here is the existence filter, the
    // same DB-candidates-can-be-dead guard the plan calls for; a row whose
    // file is gone is dropped rather than offered as an openable candidate.
    Ok(paths
        .into_iter()
        .filter_map(|raw| {
            let path = PathBuf::from(raw);
            vfs.stat(&path).ok()?;
            Some(candidate(path, root))
        })
        .collect())
}

fn candidate(path: PathBuf, root: &Path) -> Candidate {
    let in_tree = !root.as_os_str().is_empty() && path.starts_with(root);
    let display = if in_tree {
        path.strip_prefix(root).map_or_else(
            |_| path.display().to_string(),
            |rel| rel.display().to_string(),
        )
    } else {
        path.display().to_string()
    };
    Candidate {
        path,
        display,
        in_tree,
        mru_rank: None,
    }
}
