use super::*;

#[test]
fn leading_backtick_run_refuses_a_run_longer_than_want() {
    let content = "```x";
    let line = ByteRange::new(0, content.len());
    assert_eq!(leading_backtick_run(content, line, 2), None);
}

#[test]
fn leading_backtick_run_accepts_an_exact_run() {
    let content = "``x";
    let line = ByteRange::new(0, content.len());
    assert_eq!(
        leading_backtick_run(content, line, 2),
        Some(ByteRange::new(0, 2))
    );
}

#[test]
fn trailing_backtick_run_refuses_when_no_run_of_want_length_exists_in_bounds() {
    let content = "`x``y";
    assert_eq!(trailing_backtick_run(content, 1, content.len(), 1), None);
}

#[test]
fn trailing_backtick_run_finds_the_first_matching_run() {
    let content = "`ab`cd`";
    assert_eq!(
        trailing_backtick_run(content, 1, content.len(), 1),
        Some(ByteRange::new(3, 4))
    );
}

#[test]
fn trailing_backtick_run_never_scans_past_its_limit() {
    let content = "`ab`cd`";
    assert_eq!(trailing_backtick_run(content, 1, 3, 1), None);
}
