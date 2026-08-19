use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};

use crate::app::App;
use crate::fuzzymatch;
use crate::registry::{self, Availability, CommandSpec};

use super::{PaletteRow, PaletteState, Tier};

pub(super) fn rank(app: &App, state: &mut PaletteState) {
    let PaletteState {
        field,
        matcher,
        recents,
        rows,
        ..
    } = state;
    let query = field.text();
    *rows = if query.trim().is_empty() {
        empty_rows(app, recents)
    } else {
        scored_rows(app, query, matcher)
    };
}

fn tier_rank(tier: Tier) -> u8 {
    match tier {
        Tier::Recent => 0,
        Tier::NameHit => 1,
        Tier::HelpHit => 2,
        Tier::Unavailable => 3,
    }
}

fn listed_specs() -> impl Iterator<Item = &'static CommandSpec> {
    registry::rows::registry().iter().filter(|spec| spec.listed)
}

fn row_name(id: crate::registry::CommandId) -> &'static str {
    registry::spec(id).map_or("", |spec| spec.name)
}

fn scored_rows(app: &App, query: &str, matcher: &mut nucleo_matcher::Matcher) -> Vec<PaletteRow> {
    let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
    let mut scored: Vec<(PaletteRow, u32)> = Vec::new();
    for spec in listed_specs() {
        let Some((tier, score, via_alias, indices)) = score_row(spec, &pattern, matcher) else {
            continue;
        };
        let availability = (spec.availability)(app);
        let tier = if matches!(availability, Availability::Available) {
            tier
        } else {
            Tier::Unavailable
        };
        scored.push((
            PaletteRow {
                id: spec.id,
                via_alias,
                indices,
                availability,
                tier,
            },
            score,
        ));
    }
    scored.sort_by(|(a, a_score), (b, b_score)| {
        tier_rank(a.tier)
            .cmp(&tier_rank(b.tier))
            .then_with(|| b_score.cmp(a_score))
            .then_with(|| row_name(a.id).cmp(row_name(b.id)))
    });
    scored.into_iter().map(|(row, _)| row).collect()
}

fn score_row(
    spec: &CommandSpec,
    pattern: &Pattern,
    matcher: &mut nucleo_matcher::Matcher,
) -> Option<(Tier, u32, Option<&'static str>, Vec<u32>)> {
    let mut buf: Vec<char> = Vec::new();
    if let Some(score) = fuzzymatch::score(spec.name, pattern, matcher, &mut buf) {
        let indices = fuzzymatch::indices(spec.name, pattern, matcher, &mut buf);
        return Some((Tier::NameHit, score, None, indices));
    }

    let mut best_alias: Option<(&'static str, u32)> = None;
    for alias in spec.fuzzy_aliases {
        if let Some(score) = fuzzymatch::score(alias, pattern, matcher, &mut buf)
            && best_alias.is_none_or(|(_, best)| score > best)
        {
            best_alias = Some((alias, score));
        }
    }
    if let Some((alias, score)) = best_alias {
        let indices = fuzzymatch::indices(alias, pattern, matcher, &mut buf);
        return Some((Tier::NameHit, score, Some(alias), indices));
    }

    if let Some(score) = fuzzymatch::score(spec.help, pattern, matcher, &mut buf) {
        let indices = fuzzymatch::indices(spec.help, pattern, matcher, &mut buf);
        return Some((Tier::HelpHit, score, None, indices));
    }

    None
}

fn empty_rows(app: &App, recents: &[String]) -> Vec<PaletteRow> {
    let mut rows = Vec::new();
    let mut seen = Vec::new();
    for name in recents.iter().take(super::RECENTS_LIMIT as usize) {
        if let Some(spec) = listed_specs().find(|spec| spec.name == name) {
            let availability = (spec.availability)(app);
            rows.push(PaletteRow {
                id: spec.id,
                via_alias: None,
                indices: Vec::new(),
                availability,
                tier: Tier::Recent,
            });
            seen.push(spec.id);
        }
    }
    let mut rest: Vec<PaletteRow> = listed_specs()
        .filter(|spec| !seen.contains(&spec.id))
        .map(|spec| {
            let availability = (spec.availability)(app);
            let tier = if matches!(availability, Availability::Available) {
                Tier::NameHit
            } else {
                Tier::Unavailable
            };
            PaletteRow {
                id: spec.id,
                via_alias: None,
                indices: Vec::new(),
                availability,
                tier,
            }
        })
        .collect();
    rest.sort_by(|a, b| {
        tier_rank(a.tier)
            .cmp(&tier_rank(b.tier))
            .then_with(|| row_name(a.id).cmp(row_name(b.id)))
    });
    rows.extend(rest);
    rows
}
