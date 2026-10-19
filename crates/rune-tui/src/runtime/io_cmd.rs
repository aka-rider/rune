use std::path::PathBuf;
use std::sync::Arc;

use rune_vfs::Vfs;

use super::{Cmd, CmdError, DirCause, Msg, RecentsResult};

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

pub fn read_file_cmd(
    vfs: Arc<dyn Vfs + Send + Sync>,
    path: PathBuf,
    anchor: Option<rune_nav::Anchor>,
) -> Cmd {
    Cmd::read_file(move || {
        let result = rune_vfs::get(vfs.as_ref(), &path, rune_vfs::MAX_DOCUMENT_BYTES)
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

pub fn load_search_history_cmd(
    reader: rune_db::ReaderQuery,
    generation: crate::generation::SearchHistoryGen,
    doc: Option<rune_db::DocId>,
) -> Cmd {
    let request = match doc {
        Some(doc_id) => rune_db::ReaderRequestKind::RecentDocSearches { doc_id, limit: 200 },
        None => rune_db::ReaderRequestKind::RecentSearches { limit: 200 },
    };
    Cmd::search_history(move || {
        let result = reader
            .query(request)
            .map(|reply| match reply {
                rune_db::ReaderReply::RecentSearches(entries)
                | rune_db::ReaderReply::RecentDocSearches(entries) => entries,
                _ => Vec::new(),
            })
            .map_err(CmdError::from);
        Some(Msg::RecentsLoaded {
            generation: generation.raw(),
            result: RecentsResult::Search(result),
        })
    })
}

pub fn load_replace_history_cmd(
    reader: rune_db::ReaderQuery,
    generation: crate::generation::ReplaceHistoryGen,
) -> Cmd {
    Cmd::search_history(move || {
        let result = reader
            .query(rune_db::ReaderRequestKind::RecentReplacements { limit: 200 })
            .map(|reply| match reply {
                rune_db::ReaderReply::RecentReplacements(entries) => entries,
                _ => Vec::new(),
            })
            .map_err(CmdError::from);
        Some(Msg::RecentsLoaded {
            generation: generation.raw(),
            result: RecentsResult::Replace(result),
        })
    })
}

pub fn load_history_scope_cmd(reader: rune_db::ReaderQuery) -> Cmd {
    Cmd::search_history(move || {
        let result = reader
            .query(rune_db::ReaderRequestKind::UiSetting {
                key: crate::find::scope::SETTING_KEY.to_string(),
            })
            .map_err(CmdError::from)
            .and_then(|reply| match reply {
                rune_db::ReaderReply::UiSetting(value) => Ok(value),
                _ => Err(CmdError::Refused("unexpected reader reply".to_string())),
            });
        Some(Msg::HistoryScopeLoaded(result))
    })
}
