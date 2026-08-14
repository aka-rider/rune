use ratatui::layout::Rect;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Region {
    frame: Rect,
    rect: Rect,
}

impl Region {
    pub fn rect(&self) -> Rect {
        self.rect
    }

    fn clamped(frame: Rect, x: u16, y: u16, width: u16, height: u16) -> Region {
        let x = x.clamp(frame.x, frame.right());
        let y = y.clamp(frame.y, frame.bottom());
        let width = width.min(frame.right().saturating_sub(x));
        let height = height.min(frame.bottom().saturating_sub(y));
        Region {
            frame,
            rect: Rect::new(x, y, width, height),
        }
    }

    #[cfg(test)]
    pub fn sub(frame: Rect, x: u16, y: u16, width: u16, height: u16) -> Region {
        Self::clamped(frame, x, y, width, height)
    }

    pub fn band_within(frame: Rect, x: u16, width: u16) -> Region {
        Self::clamped(frame, x, frame.y, width, frame.height)
    }

    pub fn row(frame: Rect, y: u16, height: u16) -> Region {
        Self::clamped(frame, frame.x, y, frame.width, height)
    }

    #[cfg(test)]
    pub fn carve_left(frame: Rect, width: u16) -> Region {
        Self::clamped(frame, frame.x, frame.y, width, frame.height)
    }

    #[cfg(test)]
    pub fn carve_right(frame: Rect, width: u16) -> Region {
        let width = width.min(frame.width);
        let x = frame.right().saturating_sub(width);
        Self::clamped(frame, x, frame.y, width, frame.height)
    }

    pub fn carve_top(frame: Rect, height: u16) -> Region {
        Self::clamped(frame, frame.x, frame.y, frame.width, height)
    }

    pub fn carve_bottom(frame: Rect, height: u16) -> Region {
        let height = height.min(frame.height);
        let y = frame.bottom().saturating_sub(height);
        Self::clamped(frame, frame.x, y, frame.width, height)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn within(inner: Rect, outer: Rect) -> bool {
        inner.x >= outer.x
            && inner.y >= outer.y
            && inner.right() <= outer.right()
            && inner.bottom() <= outer.bottom()
    }

    #[test]
    fn band_within_clamps_both_the_start_and_the_width() {
        let frame = Rect::new(0, 0, 1, 8);
        let region = Region::band_within(frame, 0, 2);
        assert!(within(region.rect(), frame));
        assert_eq!(region.rect(), Rect::new(0, 0, 1, 8));
    }

    #[test]
    fn band_within_slides_the_start_back_into_the_frame() {
        let frame = Rect::new(5, 0, 10, 4);
        let region = Region::band_within(frame, 20, 3);
        assert!(within(region.rect(), frame));
    }

    #[test]
    fn every_constructor_stays_inside_a_swept_range_of_frames_and_requests() {
        for fw in 0..8u16 {
            for fh in 0..8u16 {
                let frame = Rect::new(1, 2, fw, fh);
                for x in 0..10u16 {
                    for w in 0..10u16 {
                        assert!(within(Region::band_within(frame, x, w).rect(), frame));
                        assert!(within(Region::row(frame, x, w).rect(), frame));
                        assert!(within(Region::sub(frame, x, x, w, w).rect(), frame));
                    }
                    assert!(within(Region::carve_left(frame, x).rect(), frame));
                    assert!(within(Region::carve_right(frame, x).rect(), frame));
                    assert!(within(Region::carve_top(frame, x).rect(), frame));
                    assert!(within(Region::carve_bottom(frame, x).rect(), frame));
                }
            }
        }
    }
}
