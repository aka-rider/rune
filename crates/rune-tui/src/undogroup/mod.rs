use rune_core::undo::{EditKind, Journal, Step};

pub const MULTI_WORD_WORDS: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    Rune,
    Word,
    MultiWord,
    Sentence,
    Line,
}

pub fn tier_for(press_index: usize) -> Tier {
    match press_index {
        0 => Tier::Rune,
        1 => Tier::Word,
        2 => Tier::MultiWord,
        3 => Tier::Sentence,
        _ => Tier::Line,
    }
}

fn is_isolated(kind: EditKind) -> bool {
    matches!(kind, EditKind::Paste | EditKind::Cut)
}

fn step_text(step: &Step) -> String {
    step.edits.iter().map(|e| e.insert.as_str()).collect()
}

fn crosses_boundary(tier: Tier, text: &str) -> bool {
    match tier {
        Tier::Rune => false,
        Tier::Word | Tier::MultiWord => text.chars().any(char::is_whitespace),
        Tier::Sentence => text.chars().any(|c| matches!(c, '.' | '!' | '?' | '\n')),
        Tier::Line => text.contains('\n'),
    }
}

fn crossings_needed(tier: Tier) -> usize {
    match tier {
        Tier::MultiWord => MULTI_WORD_WORDS,
        _ => 1,
    }
}

pub fn steps_for(journal: &Journal, tier: Tier) -> usize {
    let pos = journal.pos();
    if pos == 0 {
        return 0;
    }
    let steps = journal.steps();
    let Some(first) = steps.get(pos - 1) else {
        return 0;
    };
    if tier == Tier::Rune || is_isolated(first.kind) {
        return 1;
    }

    let target = crossings_needed(tier);
    let mut crossings = 0;
    let mut count = 1;
    let mut idx = pos - 1;

    while idx > 0 {
        let next_idx = idx - 1;
        let Some(step) = steps.get(next_idx) else {
            break;
        };
        if step.kind != first.kind || is_isolated(step.kind) {
            break;
        }
        count += 1;
        idx = next_idx;
        if crosses_boundary(tier, &step_text(step)) {
            crossings += 1;
            if crossings >= target {
                break;
            }
        }
    }

    count.min(pos)
}

#[cfg(test)]
mod tests;
