use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;

pub fn fill_row(frame: &mut Frame, row: Rect, style: Style) {
    frame.buffer_mut().set_style(row, style);
}
