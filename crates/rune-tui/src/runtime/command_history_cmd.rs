use super::{Cmd, CmdError, Msg};

pub fn load_command_history_cmd(
    reader: rune_db::ReaderQuery,
    generation: crate::generation::Generation,
) -> Cmd {
    Cmd::search_history(move || {
        let result = load(&reader);
        Some(Msg::PaletteRecentsLoaded { generation, result })
    })
}

fn load(reader: &rune_db::ReaderQuery) -> Result<Vec<String>, CmdError> {
    let reply = reader.query(rune_db::ReaderRequestKind::RecentCommands {
        limit: crate::palette::RECENTS_LIMIT,
    })?;
    match reply {
        rune_db::ReaderReply::RecentCommands(names) => Ok(names),
        rune_db::ReaderReply::Pong
        | rune_db::ReaderReply::Blob(_)
        | rune_db::ReaderReply::RecentSearches(_)
        | rune_db::ReaderReply::RecentDocuments(_) => {
            Err(CmdError::Refused("unexpected reader reply".to_string()))
        }
    }
}
