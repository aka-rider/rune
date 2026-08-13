use super::*;
use rune_core::buffer::AppliedEdit;

fn step(kind: EditKind, text: &str) -> Step {
    Step {
        edits: vec![AppliedEdit {
            start: 0,
            end: 0,
            deleted: String::new(),
            insert: text.to_string(),
        }],
        kind,
        ..Default::default()
    }
}

fn journal_of(steps: &[Step]) -> Journal {
    let mut journal = Journal::new();
    for s in steps {
        journal.push(s.clone());
    }
    journal
}

#[test]
fn tier_for_saturates_at_line() {
    assert_eq!(tier_for(0), Tier::Rune);
    assert_eq!(tier_for(1), Tier::Word);
    assert_eq!(tier_for(2), Tier::MultiWord);
    assert_eq!(tier_for(3), Tier::Sentence);
    assert_eq!(tier_for(4), Tier::Line);
    assert_eq!(tier_for(5), Tier::Line);
    assert_eq!(tier_for(1000), Tier::Line);
}

#[test]
fn rune_returns_one_step() {
    let journal = journal_of(&[
        step(EditKind::Insert, "h"),
        step(EditKind::Insert, "e"),
        step(EditKind::Insert, "l"),
    ]);
    assert_eq!(steps_for(&journal, Tier::Rune), 1);
}

#[test]
fn word_press_stops_at_the_word_boundary_after_hello_world() {
    let chars = "hello world";
    let steps: Vec<Step> = chars
        .chars()
        .map(|c| step(EditKind::Insert, &c.to_string()))
        .collect();
    let journal = journal_of(&steps);

    assert_eq!(
        steps_for(&journal, Tier::Word),
        6,
        "one Word press must undo the trailing word plus the whitespace \
         that separates it from the previous one, and no further"
    );
}

#[test]
fn line_press_stops_at_a_newline() {
    let journal = journal_of(&[
        step(EditKind::Insert, "a"),
        step(EditKind::Insert, "b"),
        step(EditKind::Insert, "\n"),
        step(EditKind::Insert, "c"),
        step(EditKind::Insert, "d"),
    ]);
    assert_eq!(
        steps_for(&journal, Tier::Line),
        3,
        "a Line press must stop once it has pulled in the newline itself"
    );
}

#[test]
fn kind_switch_stops_the_walk_short_of_the_tier() {
    let journal = journal_of(&[
        step(EditKind::DeleteLeft, ""),
        step(EditKind::Insert, "a"),
        step(EditKind::Insert, "b"),
    ]);
    assert_eq!(
        steps_for(&journal, Tier::Line),
        2,
        "the walk must stop before crossing into the DeleteLeft step, \
         even though no newline was ever found to satisfy Line"
    );
    assert!(steps_for(&journal, Tier::Line) < journal.pos());
}

#[test]
fn paste_step_is_never_absorbed_into_a_neighbouring_group() {
    let journal = journal_of(&[step(EditKind::Paste, "ab"), step(EditKind::Paste, "cd")]);
    assert_eq!(
        steps_for(&journal, Tier::Line),
        1,
        "a Paste step is an isolated unit even next to another Paste step"
    );
}

fn undo_all(journal: &mut Journal) {
    while let Some((_, token)) = journal.undo_peek() {
        journal.commit(token);
    }
}

#[test]
fn redo_rune_returns_one_step() {
    let mut journal = journal_of(&[
        step(EditKind::Insert, "h"),
        step(EditKind::Insert, "e"),
        step(EditKind::Insert, "l"),
    ]);
    undo_all(&mut journal);
    assert_eq!(steps_for_redo(&journal, Tier::Rune), 1);
}

#[test]
fn redo_word_press_mirrors_the_forward_word_boundary() {
    let chars = "hello world";
    let steps: Vec<Step> = chars
        .chars()
        .map(|c| step(EditKind::Insert, &c.to_string()))
        .collect();
    let mut journal = journal_of(&steps);
    undo_all(&mut journal);

    assert_eq!(
        steps_for_redo(&journal, Tier::Word),
        6,
        "one Word redo must replay the leading word plus the whitespace \
         that separates it from the next one, mirroring the undo direction"
    );
}

#[test]
fn redo_line_press_stops_at_a_newline() {
    let mut journal = journal_of(&[
        step(EditKind::Insert, "a"),
        step(EditKind::Insert, "b"),
        step(EditKind::Insert, "\n"),
        step(EditKind::Insert, "c"),
        step(EditKind::Insert, "d"),
    ]);
    undo_all(&mut journal);
    assert_eq!(
        steps_for_redo(&journal, Tier::Line),
        3,
        "a Line redo must stop once it has replayed the newline itself"
    );
}

#[test]
fn redo_paste_step_is_never_absorbed_into_a_neighbouring_group() {
    let mut journal = journal_of(&[step(EditKind::Paste, "ab"), step(EditKind::Paste, "cd")]);
    undo_all(&mut journal);
    assert_eq!(
        steps_for_redo(&journal, Tier::Line),
        1,
        "a Paste step is an isolated unit even next to another Paste step"
    );
}

#[test]
fn steps_for_redo_never_exceeds_the_remaining_journal() {
    let mut journal = journal_of(&[
        step(EditKind::Insert, "a"),
        step(EditKind::Insert, "b"),
        step(EditKind::Insert, "c"),
        step(EditKind::Insert, "d"),
        step(EditKind::Insert, "e"),
    ]);
    undo_all(&mut journal);
    let remaining = journal.steps().len() - journal.pos();
    for tier in [
        Tier::Rune,
        Tier::Word,
        Tier::MultiWord,
        Tier::Sentence,
        Tier::Line,
    ] {
        assert!(steps_for_redo(&journal, tier) <= remaining);
    }
    assert_eq!(steps_for_redo(&journal, Tier::Line), remaining);
}

#[test]
fn steps_for_never_exceeds_journal_pos() {
    let journal = journal_of(&[
        step(EditKind::Insert, "a"),
        step(EditKind::Insert, "b"),
        step(EditKind::Insert, "c"),
        step(EditKind::Insert, "d"),
        step(EditKind::Insert, "e"),
    ]);
    for tier in [
        Tier::Rune,
        Tier::Word,
        Tier::MultiWord,
        Tier::Sentence,
        Tier::Line,
    ] {
        assert!(steps_for(&journal, tier) <= journal.pos());
    }
    assert_eq!(steps_for(&journal, Tier::Line), journal.pos());
}
