//! List navigation and scroll-follow helpers for cursor-driven lists.

use std::ops::Range;

/// A cursor-aware list navigator, tracking the visible window top.
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

    /// Adjusts `top` so that `cursor` stays within the visible window
    /// [top + margin .. top + height - 1 - margin], with a jump buffer.
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

    /// Returns the visible index range [top, top+height) clamped to [0, len).
    pub fn window(&self, len: usize, height: usize) -> Range<usize> {
        if height == 0 || len == 0 {
            return 0..0;
        }
        let start = self.top.min(len);
        let end = (start + height).min(len);
        start..end
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
        // Scenario 1: cursor=2, top=0 — cursor is already in visible window [1,3]
        // follow should not change top
        let mut list = List { cursor: 2, top: 0 };
        list.follow(20, 5, 1, 1);
        assert_eq!(list.top, 0);

        // Scenario 2: cursor=12, top=0 — cursor is below visible window [1,3]
        // follow should scroll top down so cursor 12 is visible
        // offset = cursor - (size-1-margin) + jump = 12 - 3 + 1 = 10
        // clamped to [0, 15] = 10
        list.cursor = 12;
        list.follow(20, 5, 1, 1);
        assert_eq!(list.top, 10);
        // visible window = [11, 13], cursor 12 is inside
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
        // top=17, height=5 → 17..20 (clamped to len=20)
        assert_eq!(w, 17..20);

        let w = list.window(18, 5);
        // top=17, height=5 → 17..18 (clamped to len=18)
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
        // top must be reset to 0 when len or height is 0, not left stale
        let mut list = List { cursor: 0, top: 9 };
        list.follow(0, 5, 1, 0);
        assert_eq!(list.top, 0);
    }

    #[test]
    fn window_start_clamped_past_end() {
        // When top > len, start must be clamped to len, not produce backwards range
        let list = List { cursor: 0, top: 17 };
        let w = list.window(10, 5);
        assert_eq!(w, 10..10);
    }
}
