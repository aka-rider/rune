use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListCommand {
    Type,
    Erase,
    Up,
    Down,
    PageUp,
    PageDown,
    Top,
    Bottom,
    Enter,
    Tab,
    Cancel,
}

pub struct List {
    pub cursor: usize,
    pub top: usize,
}

impl List {
    pub fn move_by(&mut self, delta: isize, len: usize) {
        if len == 0 {
            self.cursor = 0;
            return;
        }
        self.cursor = self.cursor.saturating_add_signed(delta).min(len - 1);
    }

    pub fn first(&mut self) {
        self.cursor = 0;
    }

    pub fn last(&mut self, len: usize) {
        if len == 0 {
            self.cursor = 0;
        } else {
            self.cursor = len - 1;
        }
    }

    // Keeps `cursor` within [top + margin, top + height - 1 - margin],
    // sliding `top` by an extra `jump` rows when the cursor forces a move.
    pub fn follow(&mut self, len: usize, height: usize, margin: usize, jump: usize) {
        if len == 0 || height == 0 {
            self.top = 0;
            return;
        }

        let m = if margin * 2 > height {
            height.saturating_sub(1) / 2
        } else {
            margin
        };

        let j = jump.min(height.saturating_sub(1 + 2 * m));

        let max_offset = len.saturating_sub(height);

        let mut offset = self.top;
        let cursor = self.cursor;

        if cursor < offset + m {
            offset = cursor.saturating_sub(m + j);
        } else if cursor >= offset + height.saturating_sub(1 + m) {
            offset = cursor.saturating_sub(height.saturating_sub(1 + m)) + j;
        }

        self.top = offset.min(max_offset);
    }

    pub fn scroll_by(&mut self, delta: isize, len: usize, height: usize) {
        if len == 0 || height == 0 {
            self.top = 0;
            return;
        }
        self.top = self
            .top
            .saturating_add_signed(delta)
            .min(len.saturating_sub(height));
    }

    pub fn window(&self, len: usize, height: usize) -> Range<usize> {
        if height == 0 || len == 0 {
            return 0..0;
        }
        let start = self.top.min(len);
        let end = (start + height).min(len);
        start..end
    }

    pub(crate) fn move_and_follow(&mut self, delta: isize, len: usize, height: usize) {
        let margin = (height / 4).min(4);
        self.move_by(delta, len);
        self.follow(len, height, margin, 0);
    }

    pub(crate) fn settle(&mut self, len: usize, height: usize) {
        let margin = (height / 4).min(4);
        self.follow(len, height, margin, 0);
    }

    pub(crate) fn jump_to_edge(&mut self, len: usize, top: bool) {
        if top {
            self.first();
        } else {
            self.last(len);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamping_at_bottom() {
        let mut list = List { cursor: 0, top: 0 };
        list.move_by(10, 5);
        assert_eq!(list.cursor, 4);
    }

    #[test]
    fn clamping_at_top() {
        let mut list = List { cursor: 4, top: 0 };
        list.move_by(-10, 5);
        assert_eq!(list.cursor, 0);
    }

    #[test]
    fn empty_list_safety() {
        let mut list = List { cursor: 0, top: 0 };
        list.move_by(10, 0);
        assert_eq!(list.cursor, 0);
        list.first();
        assert_eq!(list.cursor, 0);
        list.last(0);
        assert_eq!(list.cursor, 0);
    }

    #[test]
    fn first_moves_to_zero() {
        let mut list = List { cursor: 4, top: 0 };
        list.first();
        assert_eq!(list.cursor, 0);
    }

    #[test]
    fn last_moves_to_len_minus_one() {
        let mut list = List { cursor: 0, top: 0 };
        list.last(5);
        assert_eq!(list.cursor, 4);
    }

    #[test]
    fn follow_keeps_cursor_in_window() {
        let mut list = List { cursor: 2, top: 0 };
        list.follow(20, 5, 1, 1);
        assert_eq!(list.top, 0);

        list.cursor = 12;
        list.follow(20, 5, 1, 1);
        assert_eq!(list.top, 10);
        assert!(list.cursor > list.top);
        assert!(list.cursor < list.top + 5 - 1);
    }

    #[test]
    fn window_truncation_at_end() {
        let list = List {
            cursor: 18,
            top: 17,
        };
        let w = list.window(20, 5);
        assert_eq!(w, 17..20);

        let w = list.window(18, 5);
        assert_eq!(w, 17..18);
    }

    #[test]
    fn follow_margin_boundary_height10_margin5() {
        let mut list = List { cursor: 5, top: 0 };
        list.follow(100, 10, 5, 0);
        assert_eq!(list.top, 1);
    }

    #[test]
    fn follow_resets_top_on_empty_list() {
        let mut list = List { cursor: 0, top: 9 };
        list.follow(0, 5, 1, 0);
        assert_eq!(list.top, 0);
    }

    #[test]
    fn scroll_down_clamps_at_the_last_full_window() {
        let mut list = List { cursor: 3, top: 0 };
        list.scroll_by(100, 20, 5);
        assert_eq!(list.top, 15);
    }

    #[test]
    fn scroll_up_clamps_at_the_start() {
        let mut list = List { cursor: 3, top: 4 };
        list.scroll_by(-100, 20, 5);
        assert_eq!(list.top, 0);
    }

    #[test]
    fn scrolling_never_moves_the_cursor() {
        let mut list = List { cursor: 3, top: 0 };
        list.scroll_by(9, 20, 5);
        assert_eq!(list.cursor, 3);
        list.scroll_by(-9, 20, 5);
        assert_eq!(list.cursor, 3);
    }

    #[test]
    fn scroll_resets_top_on_an_empty_list() {
        let mut list = List { cursor: 0, top: 9 };
        list.scroll_by(1, 0, 5);
        assert_eq!(list.top, 0);
        list.top = 9;
        list.scroll_by(1, 20, 0);
        assert_eq!(list.top, 0);
    }

    #[test]
    fn window_start_clamped_past_end() {
        let list = List { cursor: 0, top: 17 };
        let w = list.window(10, 5);
        assert_eq!(w, 10..10);
    }
}
