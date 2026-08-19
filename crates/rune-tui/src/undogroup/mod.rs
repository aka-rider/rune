use std::time::Duration;

use rune_core::undo::{EditKind, Journal, Step};

pub const MULTI_WORD_WORDS: usize = 3;
pub const LADDER_RESET: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Undo,
    Redo,
}

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
    matches!(
        kind,
        EditKind::Paste | EditKind::Cut | EditKind::StripTrailingWhitespace
    )
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

fn walk(
    steps: &[Step],
    first_kind: EditKind,
    tier: Tier,
    indices: impl Iterator<Item = usize>,
    cap: usize,
) -> usize {
    let target = crossings_needed(tier);
    let mut crossings = 0;
    let mut count = 1;

    for idx in indices {
        let Some(step) = steps.get(idx) else {
            break;
        };
        if step.kind != first_kind || is_isolated(step.kind) {
            break;
        }
        count += 1;
        if crosses_boundary(tier, &step_text(step)) {
            crossings += 1;
            if crossings >= target {
                break;
            }
        }
    }

    count.min(cap)
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
    walk(steps, first.kind, tier, (0..pos - 1).rev(), pos)
}

pub fn steps_for_redo(journal: &Journal, tier: Tier) -> usize {
    let pos = journal.pos();
    let steps = journal.steps();
    let len = steps.len();
    if pos == len {
        return 0;
    }
    let Some(first) = steps.get(pos) else {
        return 0;
    };
    if tier == Tier::Rune || is_isolated(first.kind) {
        return 1;
    }
    walk(steps, first.kind, tier, pos + 1..len, len - pos)
}

#[cfg(test)]
mod tests;
