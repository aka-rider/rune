const PAIRS: [(u8, u8); 3] = [(b'(', b')'), (b'[', b']'), (b'{', b'}')];

fn is_bracket(b: u8) -> bool {
    PAIRS.iter().any(|p| p.0 == b || p.1 == b)
}

fn scan_forward(bytes: &[u8], from: usize, open: u8, close: u8) -> Option<usize> {
    let mut depth = 0usize;
    for (offset, &b) in bytes.iter().enumerate().skip(from) {
        if b == open {
            depth = depth.saturating_add(1);
        } else if b == close {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return Some(offset);
            }
        }
    }
    None
}

fn scan_backward(bytes: &[u8], from: usize, open: u8, close: u8) -> Option<usize> {
    let mut depth = 0usize;
    for (offset, &b) in bytes.iter().enumerate().take(from.saturating_add(1)).rev() {
        if b == close {
            depth = depth.saturating_add(1);
        } else if b == open {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return Some(offset);
            }
        }
    }
    None
}

pub fn bracket_pair(text: &str, offset: usize) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();
    let here = *bytes.get(offset)?;
    if let Some(pair) = PAIRS.iter().find(|p| p.0 == here) {
        return scan_forward(bytes, offset, pair.0, pair.1).map(|close| (offset, close));
    }
    if let Some(pair) = PAIRS.iter().find(|p| p.1 == here) {
        return scan_backward(bytes, offset, pair.0, pair.1).map(|open| (open, offset));
    }
    None
}

pub fn pair_at_caret(text: &str, offset: usize) -> Option<(usize, usize)> {
    bracket_pair(text, offset).or_else(|| bracket_pair(text, offset.checked_sub(1)?))
}

pub fn jump_origin(text: &str, offset: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    if bytes.get(offset).copied().is_some_and(is_bracket) {
        return Some(offset);
    }
    if offset > 0 && bytes.get(offset - 1).copied().is_some_and(is_bracket) {
        return Some(offset - 1);
    }
    bytes
        .iter()
        .enumerate()
        .skip(offset)
        .take_while(|&(_, &b)| b != b'\n')
        .find(|&(_, &b)| is_bracket(b))
        .map(|(at, _)| at)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_same_type_pairs_match_by_depth() {
        let text = "( ( ) )";
        assert_eq!(bracket_pair(text, 0), Some((0, 6)));
        assert_eq!(bracket_pair(text, 2), Some((2, 4)));
        assert_eq!(bracket_pair(text, 4), Some((2, 4)));
        assert_eq!(bracket_pair(text, 6), Some((0, 6)));
    }

    #[test]
    fn mixed_pairs_count_only_their_own_type() {
        let text = "([)]";
        assert_eq!(bracket_pair(text, 0), Some((0, 2)));
        assert_eq!(bracket_pair(text, 1), Some((1, 3)));
    }

    #[test]
    fn an_unmatched_bracket_has_no_pair() {
        assert_eq!(bracket_pair("( a b", 0), None);
        assert_eq!(bracket_pair("a b )", 4), None);
    }

    #[test]
    fn an_offset_off_any_bracket_has_no_pair() {
        assert_eq!(bracket_pair("(ab)", 1), None);
        assert_eq!(bracket_pair("(ab)", 9), None);
    }

    #[test]
    fn multibyte_neighbours_keep_byte_offsets_exact() {
        let text = "é(é)é";
        let open = "é".len();
        let close = open + 1 + "é".len();
        assert_eq!(text.get(open..=close), Some("(é)"));
        assert_eq!(bracket_pair(text, open), Some((open, close)));
        assert_eq!(bracket_pair(text, close), Some((open, close)));
    }

    #[test]
    fn jump_origin_returns_the_offset_it_already_sits_on() {
        assert_eq!(jump_origin("a(b)", 1), Some(1));
        assert_eq!(jump_origin("a(b)", 3), Some(3));
    }

    #[test]
    fn jump_origin_scans_forward_within_the_line_only() {
        assert_eq!(jump_origin("a b (c)", 0), Some(4));
        assert_eq!(jump_origin("a b\n(c)", 0), None);
    }

    #[test]
    fn empty_text_has_neither_a_pair_nor_an_origin() {
        assert_eq!(bracket_pair("", 0), None);
        assert_eq!(jump_origin("", 0), None);
    }

    #[test]
    fn pair_at_caret_finds_the_pair_when_just_after_a_closing_bracket() {
        assert_eq!(pair_at_caret("(a)", 3), Some((0, 2)));
    }

    #[test]
    fn pair_at_caret_finds_the_pair_when_just_after_an_opening_bracket() {
        assert_eq!(pair_at_caret("(a)", 1), Some((0, 2)));
    }

    #[test]
    fn pair_at_caret_at_offset_zero_on_a_non_bracket_has_no_pair() {
        assert_eq!(pair_at_caret("a(b)", 0), None);
    }

    #[test]
    fn pair_at_caret_has_no_pair_when_neither_neighbouring_byte_is_a_bracket() {
        assert_eq!(pair_at_caret("a b c", 2), None);
    }

    #[test]
    fn pair_at_caret_on_a_utf8_continuation_byte_has_no_pair_and_does_not_panic() {
        assert_eq!(pair_at_caret("é)", 1), None);
        assert_eq!(pair_at_caret("(é", 2), None);
    }

    #[test]
    fn pair_at_caret_prefers_the_pair_at_the_caret_over_the_one_before_it() {
        assert_eq!(pair_at_caret("()", 1), Some((0, 1)));
    }

    #[test]
    fn jump_origin_prefers_the_bracket_just_behind_the_caret_over_a_later_one_on_the_line() {
        assert_eq!(jump_origin("(a) (b)", 3), Some(2));
    }
}
