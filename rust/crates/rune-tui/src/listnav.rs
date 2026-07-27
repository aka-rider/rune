//! List navigation — port of pkg/ui/listnav/listnav.go + scroll.go.

use std::ops::Range;

/// A cursor-aware list navigator, tracking the visible window top.
pub struct List {
    pub cursor: usize,
    pub top: usize,
}

impl List {
    /// Port of pkg/ui/listnav/listnav.go Move.
    pub fn move_by(&mut self, delta: isize, len: usize) {
        if len == 0 {
            self.cursor = 0;
            return;
        }
        let pos = self.cursor as isize + delta;
        let pos = pos.clamp(0, (len - 1) as isize);
        self.cursor = pos as usize;
    }

    /// Port of pkg/ui/listnav/listnav.go First.
    pub fn first(&mut self) {
        self.cursor = 0;
    }

    /// Port of pkg/ui/listnav/listnav.go Last.
    pub fn last(&mut self, len: usize) {
        if len == 0 {
            self.cursor = 0;
        } else {
            self.cursor = len - 1;
        }
    }

    /// Port of pkg/ui/listnav/scroll.go Follow, inlined here.
    /// Adjusts `top` so that `cursor` stays within the visible window
    /// [top + margin .. top + height - 1 - margin], with a jump buffer.
    pub fn follow(&mut self, len: usize, height: usize, margin: usize, jump: usize) {
        if len == 0 || height == 0 {
            return;
        }

        let size = height as isize;
        let total = len as isize;
        let mut offset = self.top as isize;
        let cursor = self.cursor as isize;

        if size <= 0 {
            return;
        }

        // Clamp margin to (size-1)/2 if margin*2 > size
        let mut m = margin as isize;
        if m * 2 > size {
            m = (size - 1) / 2;
        }
        if m < 0 {
            m = 0;
        }

        // Clamp jump to size-1-2*margin, then >= 0
        let mut j = jump as isize;
        let max_jump = size - 1 - 2 * m;
        if j > max_jump {
            j = max_jump;
        }
        if j < 0 {
            j = 0;
        }

        let max_offset = (total - size).max(0);

        if cursor < offset + m {
            // Cursor is above the visible window → scroll up
            offset = cursor - m - j;
        } else if cursor >= offset + size - 1 - m {
            // Cursor is below the visible window → scroll down
            offset = cursor - (size - 1 - m) + j;
        }

        offset = offset.clamp(0, max_offset);
        self.top = offset as usize;
    }

    /// Port of pkg/ui/listnav/listnav.go Window.
    /// Returns the visible index range [top, top+height) clamped to [0, len).
    pub fn window(&self, len: usize, height: usize) -> Range<usize> {
        if height == 0 || len == 0 {
            return 0..0;
        }
        let start = self.top;
        let end = (self.top + height).min(len);
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
}
