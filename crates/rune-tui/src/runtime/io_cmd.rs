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
            .map_err(CmdError::from)
            .and_then(search_entries);
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
            .map_err(CmdError::from)
            .and_then(replacement_entries);
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

fn search_entries(reply: rune_db::ReaderReply) -> Result<Vec<String>, CmdError> {
    match reply {
        rune_db::ReaderReply::RecentSearches(entries)
        | rune_db::ReaderReply::RecentDocSearches(entries) => Ok(entries),
        _ => Err(CmdError::Refused("unexpected reader reply".to_string())),
    }
}

fn replacement_entries(reply: rune_db::ReaderReply) -> Result<Vec<String>, CmdError> {
    match reply {
        rune_db::ReaderReply::RecentReplacements(entries) => Ok(entries),
        _ => Err(CmdError::Refused("unexpected reader reply".to_string())),
    }
}

#[cfg(test)]
mod tests {
    use rune_db::ReaderReply;

    use super::*;

    #[test]
    fn a_search_history_load_refuses_a_reply_of_the_wrong_kind() {
        let wrong = ReaderReply::RecentReplacements(vec!["x".to_string()]);
        assert!(matches!(search_entries(wrong), Err(CmdError::Refused(_))));
    }

    #[test]
    fn a_replace_history_load_refuses_a_reply_of_the_wrong_kind() {
        let wrong = ReaderReply::RecentSearches(vec!["x".to_string()]);
        assert!(matches!(
            replacement_entries(wrong),
            Err(CmdError::Refused(_))
        ));
    }

    #[test]
    fn both_search_reply_kinds_carry_their_entries_through() {
        for reply in [
            ReaderReply::RecentSearches(vec!["a".to_string()]),
            ReaderReply::RecentDocSearches(vec!["a".to_string()]),
        ] {
            assert_eq!(search_entries(reply).ok(), Some(vec!["a".to_string()]));
        }
    }
}
