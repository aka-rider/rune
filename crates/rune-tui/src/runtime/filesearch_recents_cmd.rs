use std::path::{Path, PathBuf};
use std::sync::Arc;

use rune_vfs::Vfs;

use crate::filesearch::Candidate;

use super::{Cmd, CmdError, Msg, RecentsResult};

pub fn load_filesearch_recents_cmd(
    reader: rune_db::ReaderQuery,
    vfs: Arc<dyn Vfs + Send + Sync>,
    root: PathBuf,
    generation: crate::generation::FileSearchGen,
) -> Cmd {
    Cmd::search_history(move || {
        let result = load(&reader, vfs.as_ref(), &root);
        Some(Msg::RecentsLoaded {
            generation: generation.raw(),
            result: RecentsResult::FileSearch(result),
        })
    })
}

fn load(
    reader: &rune_db::ReaderQuery,
    vfs: &dyn Vfs,
    root: &Path,
) -> Result<Vec<Candidate>, CmdError> {
    let reply = reader.query(rune_db::ReaderRequestKind::RecentDocuments { limit: 100 })?;
    let paths = match reply {
        rune_db::ReaderReply::RecentDocuments(paths) => paths,
        rune_db::ReaderReply::Pong
        | rune_db::ReaderReply::Blob(_)
        | rune_db::ReaderReply::RecentSearches(_)
        | rune_db::ReaderReply::RecentCommands(_) => {
            return Err(CmdError::Refused("unexpected reader reply".to_string()));
        }
    };
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
