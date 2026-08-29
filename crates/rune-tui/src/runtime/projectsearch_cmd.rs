use std::path::{Path, PathBuf};
use std::sync::Arc;

use rune_vfs::{FileKind, Vfs};

use super::{Cmd, Msg};
use crate::filesearch::walk;
use crate::projectsearch::index::{
    Fingerprint, IndexEntry, MAX_INDEX_FILE_BYTES, ReadOutcome, is_indexable,
};
use crate::projectsearch::query::run_query;

pub(crate) fn project_scan_cmd(
    vfs: Arc<dyn Vfs + Send + Sync>,
    root: PathBuf,
    generation: crate::generation::ProjectIndexGen,
) -> Cmd {
    Cmd::project_index(move || {
        let result = match vfs.stat(&root) {
            Ok(stat) if stat.kind == FileKind::Dir => {
                let mut scan = walk::scan(vfs.as_ref(), &root);
                scan.files.retain(|path| is_indexable(path));
                Ok(scan)
            }
            Ok(_) => Err(format!("{} is not a directory", root.display())),
            Err(e) => Err(format!("workspace root {} unreadable: {e}", root.display())),
        };
        Some(Msg::ProjectIndexScanned { generation, result })
    })
}

pub(crate) fn project_read_batch_cmd(
    vfs: Arc<dyn Vfs + Send + Sync>,
    batch: Vec<(PathBuf, Option<Fingerprint>)>,
    root: PathBuf,
    generation: crate::generation::ProjectIndexGen,
) -> Cmd {
    Cmd::project_index(move || {
        let outcomes = batch
            .into_iter()
            .map(|(path, fingerprint)| read_one(vfs.as_ref(), path, fingerprint, &root))
            .collect();
        Some(Msg::ProjectIndexBatch {
            generation,
            outcomes,
        })
    })
}

pub(crate) fn project_query_cmd(
    entries: Vec<Arc<IndexEntry>>,
    overrides: Vec<(PathBuf, String)>,
    query: String,
    generation: crate::generation::ProjectSearchGen,
) -> Cmd {
    Cmd::project_query(move || {
        let (results, truncated) = run_query(&entries, &overrides, &query);
        Some(Msg::ProjectSearchQueried {
            generation,
            results,
            truncated,
        })
    })
}

fn read_one(
    vfs: &dyn Vfs,
    path: PathBuf,
    fingerprint: Option<Fingerprint>,
    root: &Path,
) -> ReadOutcome {
    let Ok(stat) = vfs.stat(&path) else {
        return ReadOutcome::Skipped(path);
    };
    if fingerprint == Some((stat.size, stat.mtime)) {
        return ReadOutcome::Unchanged(path);
    }
    let Ok(sighting) = rune_vfs::get(vfs, &path, MAX_INDEX_FILE_BYTES) else {
        return ReadOutcome::Skipped(path);
    };
    if sighting.bytes.contains(&0) {
        return ReadOutcome::Skipped(path);
    }
    let Ok(text) = String::from_utf8(sighting.bytes) else {
        return ReadOutcome::Skipped(path);
    };
    let (folded, _) = crate::search::fold_with_map(&text);
    let display = crate::filesearch::display_relative(root, &path);
    ReadOutcome::Indexed(IndexEntry {
        path,
        display,
        text,
        folded,
        size: stat.size,
        mtime: stat.mtime,
    })
}
