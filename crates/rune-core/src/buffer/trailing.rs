use super::Edit;

const TAB: u8 = b'\t';
const SPACE: u8 = b' ';
const CARRIAGE_RETURN: u8 = b'\r';

pub fn trailing_whitespace_edits(content: &str) -> Vec<Edit> {
    let mut edits = Vec::new();
    let mut line_start = 0usize;
    loop {
        let rest = content.get(line_start..).unwrap_or_default();
        let newline = rest.find('\n').map(|offset| line_start + offset);
        let content_end = line_content_end(content, line_start, newline);
        let run_start = whitespace_run_start(content, line_start, content_end);
        if run_start < content_end {
            edits.push(Edit {
                start: run_start,
                end: content_end,
                insert: String::new(),
            });
        }
        match newline {
            Some(at) => line_start = at.saturating_add(1),
            None => return edits,
        }
    }
}

fn line_content_end(content: &str, line_start: usize, newline: Option<usize>) -> usize {
    let Some(at) = newline else {
        return content.len();
    };
    let before = at.saturating_sub(1);
    if at > line_start && content.as_bytes().get(before) == Some(&CARRIAGE_RETURN) {
        before
    } else {
        at
    }
}

fn whitespace_run_start(content: &str, line_start: usize, content_end: usize) -> usize {
    let bytes = content.as_bytes();
    let mut start = content_end;
    while start > line_start {
        match bytes.get(start.saturating_sub(1)) {
            Some(&TAB | &SPACE) => start = start.saturating_sub(1),
            _ => break,
        }
    }
    start
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::buffer::{Buffer, SortedEdits};

    fn stripped(content: &str) -> String {
        let buffer = Buffer::new(content);
        let edits = SortedEdits::sort(&trailing_whitespace_edits(content));
        let (after, _) = buffer.apply_edits(&edits).expect("edits apply");
        after.content().to_string()
    }

    fn ranges(content: &str) -> Vec<(usize, usize)> {
        trailing_whitespace_edits(content)
            .iter()
            .map(|e| (e.start, e.end))
            .collect()
    }

    #[test]
    fn lf_document_loses_only_the_trailing_run() {
        assert_eq!(stripped("foo  \nbar\nbaz\t\n"), "foo\nbar\nbaz\n");
    }

    #[test]
    fn crlf_document_keeps_every_carriage_return() {
        assert_eq!(
            stripped("foo  \r\nbar\r\nbaz\t \r\n"),
            "foo\r\nbar\r\nbaz\r\n"
        );
    }

    #[test]
    fn byte_order_mark_survives_a_strip_on_its_own_line() {
        assert_eq!(stripped("\u{feff}foo  \n"), "\u{feff}foo\n");
    }

    #[test]
    fn a_leading_byte_order_mark_is_never_part_of_a_run() {
        assert_eq!(stripped("\u{feff}   \n"), "\u{feff}\n");
    }

    #[test]
    fn tabs_spaces_and_mixed_runs_all_go() {
        assert_eq!(
            stripped("tabs\t\t\nspaces   \nmixed \t \t\n"),
            "tabs\nspaces\nmixed\n"
        );
    }

    #[test]
    fn a_whitespace_only_line_becomes_empty_and_keeps_its_terminator() {
        assert_eq!(stripped("a\n   \nb\n"), "a\n\nb\n");
    }

    #[test]
    fn a_line_that_is_only_a_terminator_yields_no_edit() {
        assert!(trailing_whitespace_edits("a\n\n\nb\n").is_empty());
    }

    #[test]
    fn a_final_line_without_a_terminator_is_stripped_too() {
        assert_eq!(stripped("a\nb  "), "a\nb");
    }

    #[test]
    fn a_clean_final_line_without_a_terminator_yields_no_edit() {
        assert!(trailing_whitespace_edits("a\nb").is_empty());
    }

    #[test]
    fn an_empty_document_yields_no_edit() {
        assert!(trailing_whitespace_edits("").is_empty());
    }

    #[test]
    fn a_clean_document_yields_no_edit() {
        assert!(trailing_whitespace_edits("alpha\nbeta\r\n\ngamma").is_empty());
    }

    #[test]
    fn trailing_blank_lines_are_left_alone() {
        assert_eq!(stripped("a  \n\n\n"), "a\n\n\n");
    }

    #[test]
    fn a_multibyte_rune_before_the_run_keeps_its_bytes() {
        assert_eq!(stripped("héllo  \n"), "héllo\n");
        assert_eq!(ranges("héllo  \n"), vec![(6, 8)]);
    }

    #[test]
    fn an_emoji_before_the_run_keeps_its_bytes() {
        assert_eq!(stripped("a🎉\t\nb"), "a🎉\nb");
        assert_eq!(ranges("a🎉\t\nb"), vec![(5, 6)]);
    }

    #[test]
    fn every_edit_starts_and_ends_on_a_char_boundary() {
        let content = "héllo  \nこんにちは \t\n🎉  ";
        for edit in trailing_whitespace_edits(content) {
            assert!(content.is_char_boundary(edit.start));
            assert!(content.is_char_boundary(edit.end));
        }
    }

    #[test]
    fn a_bare_carriage_return_inside_content_is_not_a_terminator() {
        assert_eq!(stripped("a\rb  \n"), "a\rb\n");
    }

    #[test]
    fn a_bare_carriage_return_at_end_of_file_survives() {
        assert!(trailing_whitespace_edits("a\r").is_empty());
        assert_eq!(stripped("a  \rb\r"), "a  \rb\r");
    }

    #[test]
    fn a_carriage_return_left_by_a_strip_is_never_deleted() {
        assert_eq!(stripped("a\r  \n"), "a\r\n");
    }

    #[test]
    fn edits_come_back_ascending_by_start() {
        let starts: Vec<usize> = trailing_whitespace_edits("a \nbb  \nccc\t\n")
            .iter()
            .map(|e| e.start)
            .collect();
        assert_eq!(starts, vec![1, 5, 11]);
    }

    #[test]
    fn a_document_of_only_whitespace_lines_empties_every_line() {
        assert_eq!(stripped("  \n\t\n \t \n"), "\n\n\n");
    }

    #[test]
    fn every_edit_deletes_only_tabs_and_spaces() {
        let content = "a  \nb\t\r\nc \t ";
        for edit in trailing_whitespace_edits(content) {
            let run = content.get(edit.start..edit.end).expect("valid range");
            assert!(run.bytes().all(|b| b == b'\t' || b == b' '));
            assert!(edit.insert.is_empty());
        }
    }
}
