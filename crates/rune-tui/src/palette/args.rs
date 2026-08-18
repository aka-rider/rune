use nucleo_matcher::Utf32Str;
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use rune_syntax::{DocumentKind, LangId};

use crate::app::App;
use crate::document::DocumentId;
use crate::registry::{ArgKind, CommandId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LanguageChoice {
    Auto,
    Markdown,
    Plain,
    Lang(LangId),
}

#[derive(Clone, Debug, PartialEq)]
pub enum ResolvedArg {
    Language(LanguageChoice),
    Tab(DocumentId),
}

pub struct ArgRow {
    pub label: String,
    pub via_alias: Option<&'static str>,
    pub indices: Vec<u32>,
    pub current: bool,
    pub resolved: ResolvedArg,
}

struct RawCandidate {
    label: String,
    aliases: Vec<&'static str>,
    current: bool,
    resolved: ResolvedArg,
}

fn language_candidates(app: &App) -> Vec<RawCandidate> {
    let kind = app.active_doc().kind;
    let mut out = vec![
        RawCandidate {
            label: "auto".to_string(),
            aliases: Vec::new(),
            current: !app.active_doc().kind_pinned,
            resolved: ResolvedArg::Language(LanguageChoice::Auto),
        },
        RawCandidate {
            label: "markdown".to_string(),
            aliases: Vec::new(),
            current: kind == DocumentKind::Markdown,
            resolved: ResolvedArg::Language(LanguageChoice::Markdown),
        },
        RawCandidate {
            label: "plain".to_string(),
            aliases: Vec::new(),
            current: kind == DocumentKind::Plain,
            resolved: ResolvedArg::Language(LanguageChoice::Plain),
        },
    ];
    for def in rune_ts::lang::LANGUAGES {
        let Some(id) = LangId::from_name(def.name) else {
            continue;
        };
        let aliases: Vec<&'static str> = rune_ts::lang::ALIASES
            .iter()
            .filter(|(_, name)| *name == def.name)
            .map(|(alias, _)| *alias)
            .collect();
        out.push(RawCandidate {
            label: def.name.to_string(),
            aliases,
            current: kind == DocumentKind::Code(id),
            resolved: ResolvedArg::Language(LanguageChoice::Lang(id)),
        });
    }
    out
}

fn open_tab_candidates(app: &App) -> Vec<RawCandidate> {
    app.documents
        .order()
        .iter()
        .filter_map(|&id| {
            let doc = app.doc(id)?;
            Some(RawCandidate {
                label: doc.file_name().to_string(),
                aliases: Vec::new(),
                current: id == app.active,
                resolved: ResolvedArg::Tab(id),
            })
        })
        .collect()
}

fn raw_candidates(app: &App, cmd: CommandId) -> Vec<RawCandidate> {
    let Some(spec) = crate::registry::spec(cmd) else {
        return Vec::new();
    };
    match spec.arg {
        ArgKind::Language => language_candidates(app),
        ArgKind::OpenTab => open_tab_candidates(app),
        ArgKind::None => Vec::new(),
    }
}

fn score_candidate(
    candidate: &RawCandidate,
    pattern: &Pattern,
    matcher: &mut nucleo_matcher::Matcher,
) -> Option<(u32, Option<&'static str>, Vec<u32>)> {
    let mut buf: Vec<char> = Vec::new();
    if let Some(score) = pattern.score(Utf32Str::new(&candidate.label, &mut buf), matcher) {
        let mut indices = Vec::new();
        let _ = pattern.indices(
            Utf32Str::new(&candidate.label, &mut buf),
            matcher,
            &mut indices,
        );
        indices.sort_unstable();
        indices.dedup();
        return Some((score, None, indices));
    }
    let mut best: Option<(&'static str, u32)> = None;
    for &alias in &candidate.aliases {
        if let Some(score) = pattern.score(Utf32Str::new(alias, &mut buf), matcher)
            && best.is_none_or(|(_, b)| score > b)
        {
            best = Some((alias, score));
        }
    }
    best.map(|(alias, score)| (score, Some(alias), Vec::new()))
}

pub(crate) fn rank(
    app: &App,
    cmd: CommandId,
    query: &str,
    matcher: &mut nucleo_matcher::Matcher,
) -> Vec<ArgRow> {
    let raw = raw_candidates(app, cmd);
    if query.trim().is_empty() {
        let mut rows: Vec<ArgRow> = raw
            .into_iter()
            .map(|c| ArgRow {
                label: c.label,
                via_alias: None,
                indices: Vec::new(),
                current: c.current,
                resolved: c.resolved,
            })
            .collect();
        rows.sort_by(|a, b| {
            b.current
                .cmp(&a.current)
                .then_with(|| a.label.cmp(&b.label))
        });
        return rows;
    }
    let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
    let mut scored: Vec<(ArgRow, u32)> = Vec::new();
    for candidate in raw {
        let Some((score, via_alias, indices)) = score_candidate(&candidate, &pattern, matcher)
        else {
            continue;
        };
        scored.push((
            ArgRow {
                label: candidate.label,
                via_alias,
                indices,
                current: candidate.current,
                resolved: candidate.resolved,
            },
            score,
        ));
    }
    scored.sort_by(|(a, a_score), (b, b_score)| {
        b_score.cmp(a_score).then_with(|| a.label.cmp(&b.label))
    });
    scored.into_iter().map(|(row, _)| row).collect()
}

pub(crate) fn ghost_suffix(query: &str, label: &str) -> Option<String> {
    if query.is_empty() {
        return None;
    }
    let mut chars = label.chars();
    for q in query.chars() {
        match chars.next() {
            Some(l) if l.to_lowercase().eq(q.to_lowercase()) => continue,
            _ => return None,
        }
    }
    let rest: String = chars.collect();
    if rest.is_empty() { None } else { Some(rest) }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use rune_core::buffer::Buffer;
    use rune_vfs::Mem;
    use std::sync::Arc;

    fn app() -> App {
        App::new(Buffer::new("hello"), None, Arc::new(Mem::new()), None)
    }

    #[test]
    fn ghost_suffix_finds_the_prefix_remainder() {
        assert_eq!(ghost_suffix("ru", "rust"), Some("st".to_string()));
        assert_eq!(ghost_suffix("RU", "rust"), Some("st".to_string()));
    }

    #[test]
    fn ghost_suffix_is_none_for_a_scattered_match() {
        assert_eq!(ghost_suffix("rt", "rust"), None);
    }

    #[test]
    fn ghost_suffix_is_none_once_the_query_equals_the_label() {
        assert_eq!(ghost_suffix("rust", "rust"), None);
    }

    #[test]
    fn language_candidates_mark_the_documents_current_kind() {
        let app = app();
        let rows = rank(
            &app,
            CommandId::Palette(crate::registry::PaletteCommand::Language),
            "",
            &mut nucleo_matcher::Matcher::new(nucleo_matcher::Config::DEFAULT),
        );
        let markdown = rows
            .iter()
            .find(|r| r.label == "markdown")
            .expect("markdown row");
        assert!(markdown.current);
        let auto = rows.iter().find(|r| r.label == "auto").expect("auto row");
        assert!(auto.current);
    }

    #[test]
    fn tab_candidates_mark_the_active_document_current() {
        let app = app();
        let id = app.active;
        let rows = rank(
            &app,
            CommandId::Palette(crate::registry::PaletteCommand::TabByName),
            "",
            &mut nucleo_matcher::Matcher::new(nucleo_matcher::Config::DEFAULT),
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].resolved, ResolvedArg::Tab(id));
        assert!(rows[0].current);
    }

    #[test]
    fn a_language_alias_query_surfaces_the_canonical_name() {
        let app = app();
        let rows = rank(
            &app,
            CommandId::Palette(crate::registry::PaletteCommand::Language),
            "c++",
            &mut nucleo_matcher::Matcher::new(nucleo_matcher::Config::DEFAULT),
        );
        assert_eq!(rows.first().map(|r| r.label.as_str()), Some("cpp"));
        assert_eq!(rows.first().and_then(|r| r.via_alias), Some("c++"));
    }
}
